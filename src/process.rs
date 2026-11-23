//! The chunk tools' work on region files, one region at a time: filters, overlays, deleting,
//! changing and importing chunks, and exporting them as a zip. Region, entities and poi files
//! of a region are read through Wings (with the `.mcc` files of oversized chunks when needed),
//! processed off the async runtime, and written back through [`ops::Writer`] (backup first,
//! atomic per file, server stopped).
use crate::{
    anvil::{self, ChunkData, Region, RegionChunk},
    changer::{self, ChangeSet, ChunkMut},
    coords,
    filter::{ChunkContext, Filter, Needs},
    jobs::{self, Handle, Stop},
    map::Map,
    nbt::{self, Compound},
    ops::{self, OperationKind, Writer},
    overlay::Overlay,
    relocate::{self, Offset, Ranges},
    selection::{ChunkSet, Selection},
    tiles,
};
use serde_json::json;
use shared::response::ApiResponse;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};
use uuid::Uuid;

/// The three kinds of region files of a dimension, in the order MCA Selector handles them.
pub const KINDS: [&str; 3] = ["region", "entities", "poi"];
/// Errors listed in a job's result; more are counted only.
const MAX_ERRORS: usize = 50;

pub type Coords = (i32, i32);

pub fn file_path(folder: &str, kind: &str, (x, z): Coords) -> String {
    format!("{folder}/{kind}/{}", coords::region_file_name(x, z))
}

pub fn mcc_path(folder: &str, kind: &str, chunk_x: i32, chunk_z: i32) -> String {
    format!("{folder}/{kind}/{}", anvil::mcc_name(chunk_x, chunk_z))
}

/// Absolute chunk coordinates of slot `index` of region `(rx, rz)`.
pub fn chunk_at((rx, rz): Coords, index: usize) -> Coords {
    (rx * 32 + (index % 32) as i32, rz * 32 + (index / 32) as i32)
}

async fn failed(response: ApiResponse) -> Stop {
    Stop::Failed(jobs::describe(response).await)
}

fn blocking_failed<E>(_: E) -> Stop {
    Stop::Failed("processing a region failed unexpectedly".into())
}

/// MCA Selector's `isAnyChunkInRegionSelected`.
pub fn any_in_region(selection: &Selection, region: Coords) -> bool {
    match selection.regions.get(&region) {
        None => selection.inverted,
        Some(chunks) => !selection.inverted || chunks.is_some(),
    }
}

/// The slots of `region` a selection covers, `None` when it covers none (MCA Selector's
/// `getSelectedChunks`).
pub fn chunks_in_region(selection: &Selection, region: Coords) -> Option<ChunkSet> {
    let set = match (selection.regions.get(&region), selection.inverted) {
        (None, false) => return None,
        (None, true) => ChunkSet::full(),
        (Some(None), false) => ChunkSet::full(),
        (Some(None), true) => return None,
        (Some(Some(chunks)), false) => *chunks,
        (Some(Some(chunks)), true) => chunks.invert(),
    };
    (!set.is_empty()).then_some(set)
}

/// One region file, parsed, with the `.mcc` files of the oversized chunks that were read.
pub struct Loaded {
    pub path: String,
    pub region: Region,
    pub external: HashMap<usize, Vec<u8>>,
}

impl Loaded {
    pub fn decode(&self, index: usize) -> Result<Option<Compound>, String> {
        self.region
            .decode(index, self.external.get(&index).map(Vec::as_slice))
    }

    fn is_external(&self, index: usize) -> bool {
        matches!(
            self.region.get(index),
            Some(RegionChunk {
                data: ChunkData::External { .. },
                ..
            })
        )
    }
}

/// The region files of the dimension `folder` that exist, per kind.
pub struct Listing {
    pub kinds: BTreeMap<&'static str, BTreeSet<Coords>>,
}

impl Listing {
    pub async fn read(map: &Map, folder: &str) -> Result<Self, Stop> {
        let mut kinds = BTreeMap::new();
        for kind in KINDS {
            let files = match map.regions(folder, kind, true).await {
                Ok(files) => files,
                Err(err) => return Err(failed(err).await),
            };
            kinds.insert(kind, files.iter().map(|file| (file.x, file.z)).collect());
        }
        Ok(Self { kinds })
    }

    pub fn has(&self, kind: &str, region: Coords) -> bool {
        self.kinds
            .get(kind)
            .is_some_and(|set| set.contains(&region))
    }

    /// Regions with a file of any kind (what an inverted selection is resolved within).
    pub fn all(&self) -> BTreeSet<Coords> {
        self.kinds.values().flatten().copied().collect()
    }

    /// Regions with a file of one of the needed kinds; region files always count.
    pub fn needed(&self, needs: Needs) -> BTreeSet<Coords> {
        let mut set = self.kinds["region"].clone();
        if needs.entities {
            set.extend(self.kinds["entities"].iter().copied());
        }
        if needs.poi {
            set.extend(self.kinds["poi"].iter().copied());
        }
        set
    }
}

/// Reads and parses a region file (`None` when it does not exist); the `.mcc` files of the
/// oversized chunks among `externals` are read too.
pub async fn load(
    map: &Map,
    folder: &str,
    kind: &str,
    region: Coords,
    externals: Option<&ChunkSet>,
) -> Result<Option<Loaded>, Stop> {
    let path = file_path(folder, kind, region);
    let data = match map.read(&path, map.region_limit()).await {
        Ok(Some(data)) => data,
        Ok(None) => return Ok(None),
        Err(err) => return Err(failed(err).await),
    };
    let parsed = tokio::task::spawn_blocking(move || Region::parse(&data))
        .await
        .map_err(blocking_failed)?
        .map_err(|err| Stop::Failed(format!("{path} could not be read: {err}")))?;
    let mut loaded = Loaded {
        path,
        region: parsed,
        external: HashMap::new(),
    };
    if let Some(wanted) = externals {
        for index in loaded.region.occupied() {
            if !wanted.get(index) || !loaded.is_external(index) {
                continue;
            }
            let (x, z) = chunk_at(region, index);
            let mcc = mcc_path(folder, kind, x, z);
            match map.read(&mcc, map.region_limit()).await {
                Ok(Some(bytes)) => {
                    loaded.external.insert(index, bytes);
                }
                Ok(None) => {}
                Err(err) => return Err(failed(err).await),
            }
        }
    }
    Ok(Some(loaded))
}

/// The files of one region a job works on.
#[derive(Default)]
pub struct Files {
    pub region: Option<Loaded>,
    pub entities: Option<Loaded>,
    pub poi: Option<Loaded>,
}

impl Files {
    pub async fn load(
        map: &Map,
        folder: &str,
        listing: &Listing,
        region: Coords,
        needs: Needs,
        externals: Option<&ChunkSet>,
    ) -> Result<Self, Stop> {
        let mut files = Self::default();
        if listing.has("region", region) {
            // region files are always read: they tell which chunks exist
            let externals = if needs.region { externals } else { None };
            files.region = load(map, folder, "region", region, externals).await?;
        }
        if needs.entities && listing.has("entities", region) {
            files.entities = load(map, folder, "entities", region, externals).await?;
        }
        if needs.poi && listing.has("poi", region) {
            files.poi = load(map, folder, "poi", region, externals).await?;
        }
        Ok(files)
    }

    fn get(&self, kind: &str) -> Option<&Loaded> {
        match kind {
            "region" => self.region.as_ref(),
            "entities" => self.entities.as_ref(),
            _ => self.poi.as_ref(),
        }
    }

    fn present(&self, index: usize) -> bool {
        KINDS.iter().any(|kind| {
            self.get(kind)
                .is_some_and(|loaded| loaded.region.get(index).is_some())
        })
    }
}

/// Decodes what `needs` asks for at `index` and hands it to `use_chunk`; decoding errors are
/// returned as text. `neighbours` tells whether a region/ chunk exists at absolute chunk
/// coordinates (MCA Selector's Border filter).
fn with_context<T>(
    files: &Files,
    needs: Needs,
    region: Coords,
    index: usize,
    neighbours: Option<&dyn Fn(i32, i32) -> bool>,
    use_chunk: impl FnOnce(&ChunkContext) -> T,
) -> Result<T, String> {
    let decode = |loaded: Option<&Loaded>, wanted: bool| -> Result<Option<Compound>, String> {
        match loaded {
            Some(loaded) if wanted => loaded.decode(index),
            _ => Ok(None),
        }
    };
    let region_root = decode(files.region.as_ref(), needs.region)?;
    let entities_root = decode(files.entities.as_ref(), needs.entities)?;
    let poi_root = decode(files.poi.as_ref(), needs.poi)?;
    let stored = files
        .region
        .as_ref()
        .and_then(|loaded| loaded.region.get(index));
    let (x, z) = chunk_at(region, index);
    let context = ChunkContext {
        x,
        z,
        region: region_root.as_ref(),
        entities: entities_root.as_ref(),
        poi: poi_root.as_ref(),
        timestamp: stored.map_or(0, |chunk| chunk.timestamp),
        compression: stored.map(|chunk| match &chunk.data {
            ChunkData::Raw { compression, .. }
            | ChunkData::External { compression }
            | ChunkData::Nbt { compression, .. } => *compression & 0x7F,
        }),
        external: stored.is_some_and(|chunk| matches!(chunk.data, ChunkData::External { .. })),
        size: files
            .region
            .as_ref()
            .and_then(|loaded| loaded.region.stored_size(index)),
        neighbours,
    };
    Ok(use_chunk(&context))
}

fn note(errors: &mut Vec<serde_json::Value>, failures: &mut usize, region: &str, error: String) {
    *failures += 1;
    if errors.len() < MAX_ERRORS {
        errors.push(json!({ "region": region, "error": error }));
    }
}

fn region_label((x, z): Coords) -> String {
    coords::region_file_name(x, z)
}

/// Selects the chunks matching `filter` (a read-only job): MCA Selector's "Filter chunks" with
/// "select". `within` limits it to a selection.
pub async fn scan(
    handle: &Handle,
    map: &Map,
    folder: &str,
    filter: Arc<Filter>,
    within: Option<Selection>,
) -> Result<serde_json::Value, Stop> {
    let needs = filter.needs();
    let listing = Listing::read(map, folder).await?;
    let regions: Vec<Coords> = listing
        .needed(needs)
        .into_iter()
        .filter(|&(x, z)| filter.region_may_match(x, z))
        .filter(|region| within.as_ref().is_none_or(|s| any_in_region(s, *region)))
        .collect();
    // which region/ chunks exist around the region, for the Border filter; regions come in
    // x order, so columns left behind are dropped
    let mut occupancy: HashMap<Coords, ChunkSet> = HashMap::new();
    let total = regions.len() as u64;
    let mut selection = Selection::default();
    let mut matched = 0u64;
    let mut errors = Vec::new();
    let mut failures = 0usize;
    for (done, &region) in regions.iter().enumerate() {
        handle.progress(done as u64, total);
        if handle.is_cancelled() {
            break;
        }
        let wanted = match &within {
            Some(within) => match chunks_in_region(within, region) {
                Some(set) => set,
                None => continue,
            },
            None => ChunkSet::full(),
        };
        let _slot = tiles::slot(map.settings.render_concurrency)
            .await
            .map_err(|_| Stop::from("the panel is shutting down"))?;
        let files = Files::load(map, folder, &listing, region, needs, Some(&wanted)).await?;
        let around = if filter.needs_neighbours() {
            occupancy.retain(|&(x, _), _| x >= region.0 - 1);
            let mut around = HashMap::new();
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let at = (region.0 + dx, region.1 + dz);
                    if !occupancy.contains_key(&at) {
                        let mut set = ChunkSet::default();
                        let loaded = if at == region {
                            None
                        } else if listing.has("region", at) {
                            load(map, folder, "region", at, None).await?
                        } else {
                            None
                        };
                        let source = if at == region {
                            files.region.as_ref()
                        } else {
                            loaded.as_ref()
                        };
                        if let Some(source) = source {
                            for index in source.region.occupied() {
                                set.set(index);
                            }
                        }
                        occupancy.insert(at, set);
                    }
                    around.insert(at, occupancy[&at]);
                }
            }
            Some(around)
        } else {
            None
        };
        let filter = filter.clone();
        let (found, failed_chunks) = tokio::task::spawn_blocking(move || {
            let exists = |x: i32, z: i32| {
                around.as_ref().is_some_and(|around| {
                    around
                        .get(&(coords::region_of_chunk(x), coords::region_of_chunk(z)))
                        .is_some_and(|set| set.get(coords::slot(x, z)))
                })
            };
            let neighbours: Option<&dyn Fn(i32, i32) -> bool> = if around.is_some() {
                Some(&exists)
            } else {
                None
            };
            let mut found = Vec::new();
            let mut failed_chunks = Vec::new();
            // like MCA Selector, every slot is tested: a missing chunk matches filters such as
            // `InhabitedTime < 10` or `Circle`, which only selects positions, never deletes data
            for index in wanted.iter() {
                match with_context(&files, needs, region, index, neighbours, |chunk| {
                    filter.matches(chunk)
                }) {
                    Ok(true) => found.push(index),
                    Ok(false) => {}
                    Err(err) => failed_chunks.push(format!("chunk {index}: {err}")),
                }
            }
            (found, failed_chunks)
        })
        .await
        .map_err(blocking_failed)?;
        for index in found {
            let (x, z) = chunk_at(region, index);
            selection.add_chunk(x, z);
            matched += 1;
        }
        for error in failed_chunks {
            note(&mut errors, &mut failures, &region_label(region), error);
        }
    }
    handle.progress(total, total);
    Ok(json!({
        "matched": matched,
        "regions_read": regions.len(),
        "selection": selection.to_wire(),
        "errors": errors,
        "failures": failures,
        "partial": handle.is_cancelled(),
    }))
}

/// The overlay values of one region: 1024 entries, `x + z * 32`.
pub async fn overlay_values(
    map: &Map,
    folder: &str,
    listing: &Listing,
    region: Coords,
    overlay: Arc<Overlay>,
) -> Result<Vec<Option<f64>>, Stop> {
    let needs = overlay.needs();
    let files = Files::load(map, folder, listing, region, needs, Some(&ChunkSet::full())).await?;
    tokio::task::spawn_blocking(move || {
        (0..anvil::CHUNKS)
            .map(|index| {
                if !files.present(index) {
                    return None;
                }
                with_context(&files, needs, region, index, None, |chunk| {
                    overlay.value(chunk)
                })
                .ok()
                .flatten()
                .filter(|value| value.is_finite())
            })
            .collect()
    })
    .await
    .map_err(blocking_failed)
}

/// A region rewritten in memory, ready for the writer.
struct Output {
    path: String,
    kind: &'static str,
    region: Region,
    /// Slots stored externally before the change (their `.mcc` may have to go).
    external_before: BTreeSet<usize>,
}

/// Writes one changed region file and its `.mcc` files: new `.mcc` files first, then the region
/// file (or its removal when no chunk is left), then the `.mcc` files no longer referenced.
async fn write_output(
    writer: &mut Writer<'_>,
    folder: &str,
    region_coords: Coords,
    output: Output,
) -> Result<usize, Stop> {
    let Output {
        path,
        kind,
        region,
        external_before,
    } = output;
    let empty = region.is_empty();
    let written = if empty {
        None
    } else {
        Some(
            tokio::task::spawn_blocking(move || region.write())
                .await
                .map_err(blocking_failed)?
                .map_err(|err| Stop::Failed(format!("{path} could not be written: {err}")))?,
        )
    };
    let mut files = 0;
    let mut external_after = BTreeSet::new();
    let mut stubs = BTreeSet::new();
    if let Some(written) = &written {
        let parsed = Region::parse(&written.region)
            .map_err(|err| Stop::Failed(format!("{path} was written wrong: {err}")))?;
        for index in parsed.occupied() {
            if matches!(
                parsed.get(index).map(|c| &c.data),
                Some(ChunkData::External { .. })
            ) {
                stubs.insert(index);
            }
        }
    }
    if let Some(written) = written {
        for (index, bytes) in written.external {
            let (x, z) = chunk_at(region_coords, index);
            if let Err(err) = writer.replace(&mcc_path(folder, kind, x, z), bytes).await {
                return Err(failed(err).await);
            }
            external_after.insert(index);
            files += 1;
        }
        if let Err(err) = writer.replace(&path, written.region).await {
            return Err(failed(err).await);
        }
    } else if let Err(err) = writer.remove(&path).await {
        return Err(failed(err).await);
    }
    files += 1;
    // unchanged external chunks keep their stub and their .mcc
    for index in external_before {
        if external_after.contains(&index) || stubs.contains(&index) {
            continue;
        }
        let (x, z) = chunk_at(region_coords, index);
        if let Err(err) = writer.remove(&mcc_path(folder, kind, x, z)).await {
            return Err(failed(err).await);
        }
    }
    Ok(files)
}

fn externals(region: &Region) -> BTreeSet<usize> {
    region
        .occupied()
        .into_iter()
        .filter(|&index| {
            matches!(
                region.get(index).map(|c| &c.data),
                Some(ChunkData::External { .. })
            )
        })
        .collect()
}

/// What a write job tells the page.
struct Tally {
    chunks: u64,
    skipped: u64,
    files: usize,
    errors: Vec<serde_json::Value>,
    failures: usize,
}

impl Tally {
    fn new() -> Self {
        Self {
            chunks: 0,
            skipped: 0,
            files: 0,
            errors: Vec::new(),
            failures: 0,
        }
    }
}

/// Runs `step` for every region with the writer, then finishes the operation. A failure or a
/// cancellation keeps what was written (marked partial, restorable from its backup).
async fn write_job<'a, F>(
    handle: &Handle,
    map: &'a Map,
    user: Uuid,
    kind: OperationKind,
    place: (&str, &str),
    regions: Vec<Coords>,
    summary: serde_json::Value,
    mut step: F,
) -> Result<serde_json::Value, Stop>
where
    F: AsyncStep<'a>,
{
    let _lock = ops::lock(map.server).await;
    let mut writer = match Writer::begin(map, user, kind, place.0, place.1).await {
        Ok(writer) => writer,
        Err(err) => return Err(failed(err).await),
    };
    let mut tally = Tally::new();
    let total = regions.len() as u64;
    let mut outcome = Ok(());
    for (done, region) in regions.into_iter().enumerate() {
        handle.progress(done as u64, total);
        if handle.is_cancelled() {
            break;
        }
        if let Err(stop) = step.run(&mut writer, region, &mut tally).await {
            outcome = Err(stop);
            break;
        }
    }
    handle.progress(total, total);
    let mut summary = summary;
    summary["chunks"] = json!(tally.chunks);
    summary["skipped"] = json!(tally.skipped);
    let operation = match (&outcome, handle.is_cancelled()) {
        (Ok(()), false) => writer.finish(summary).await,
        (Ok(()), true) => writer.abort("cancelled").await,
        (Err(Stop::Failed(error)), _) => writer.abort(error).await,
        (Err(Stop::Cancelled), _) => writer.abort("cancelled").await,
    };
    map.forget(step.folder());
    let operation = match operation {
        Ok(operation) => operation,
        Err(err) => return Err(failed(err).await),
    };
    if let Err(Stop::Failed(error)) = outcome {
        return Err(Stop::Failed(format!(
            "{error}. What was written before is listed in the history (backup {}) and can be restored.",
            operation.backup
        )));
    }
    Ok(json!({
        "chunks": tally.chunks,
        "skipped": tally.skipped,
        "files": tally.files,
        "operation": operation,
        "errors": tally.errors,
        "failures": tally.failures,
        "partial": handle.is_cancelled(),
    }))
}

type StepFuture<'s> = std::pin::Pin<Box<dyn Future<Output = Result<(), Stop>> + Send + 's>>;

/// One region's work in a write job.
trait AsyncStep<'a> {
    fn folder(&self) -> &str;
    fn run<'s>(
        &'s mut self,
        writer: &'s mut Writer<'a>,
        region: Coords,
        tally: &'s mut Tally,
    ) -> StepFuture<'s>;
}

/// The chunks of a resolved selection, refused beyond `limit`.
fn resolve_limited(
    selection: &Selection,
    existing: &BTreeSet<Coords>,
    limit: u64,
) -> Result<BTreeMap<Coords, ChunkSet>, Stop> {
    let resolved = selection.resolve(existing);
    let count: u64 = resolved.values().map(|set| u64::from(set.count())).sum();
    if count == 0 {
        return Err(Stop::from("the selection holds no chunk of this dimension"));
    }
    if count > limit {
        return Err(Stop::Failed(format!(
            "the selection holds {count} chunks; one operation may touch at most {limit} (admin setting)"
        )));
    }
    Ok(resolved)
}

struct DeleteStep<'a> {
    map: &'a Map,
    folder: String,
    listing: Listing,
    resolved: BTreeMap<Coords, ChunkSet>,
}

impl<'a> AsyncStep<'a> for DeleteStep<'a> {
    fn folder(&self) -> &str {
        &self.folder
    }

    fn run<'s>(
        &'s mut self,
        writer: &'s mut Writer<'a>,
        region: Coords,
        tally: &'s mut Tally,
    ) -> StepFuture<'s> {
        Box::pin(async move {
            let Some(set) = self.resolved.get(&region).copied() else {
                return Ok(());
            };
            let _slot = tiles::slot(self.map.settings.render_concurrency)
                .await
                .map_err(|_| Stop::from("the panel is shutting down"))?;
            for kind in KINDS {
                if !self.listing.has(kind, region) {
                    continue;
                }
                let Some(mut loaded) = load(self.map, &self.folder, kind, region, None).await?
                else {
                    continue;
                };
                let external_before = externals(&loaded.region);
                let mut removed = 0;
                for index in set.iter() {
                    if loaded.region.set(index, None).is_some() {
                        removed += 1;
                    }
                }
                if removed == 0 {
                    continue;
                }
                if kind == "region" {
                    tally.chunks += removed;
                }
                tally.files += write_output(
                    writer,
                    &self.folder,
                    region,
                    Output {
                        path: loaded.path,
                        kind,
                        region: loaded.region,
                        external_before,
                    },
                )
                .await?;
            }
            Ok(())
        })
    }
}

/// Deletes the selected chunks from the region, entities and poi files (MCA Selector's
/// "Delete selected chunks"); the game generates them again.
pub async fn delete(
    handle: &Handle,
    map: &Map,
    user: Uuid,
    place: (&str, &str),
    folder: &str,
    selection: &Selection,
) -> Result<serde_json::Value, Stop> {
    let listing = Listing::read(map, folder).await?;
    let resolved = resolve_limited(
        selection,
        &listing.all(),
        u64::from(map.settings.max_delete_chunks),
    )?;
    let selected: u64 = resolved.values().map(|set| u64::from(set.count())).sum();
    let regions: Vec<Coords> = resolved.keys().copied().collect();
    let step = DeleteStep {
        map,
        folder: folder.to_string(),
        listing,
        resolved,
    };
    write_job(
        handle,
        map,
        user,
        OperationKind::Delete,
        place,
        regions,
        json!({ "selected": selected }),
        step,
    )
    .await
}

struct ChangeStep<'a> {
    map: &'a Map,
    folder: String,
    listing: Listing,
    resolved: BTreeMap<Coords, ChunkSet>,
    changes: Arc<ChangeSet>,
    force: bool,
}

/// The regions changed in memory and how many chunks changed.
type Changes = (
    Vec<(&'static str, Loaded, BTreeSet<usize>)>,
    u64,
    Vec<String>,
);

fn compression_of(chunk: Option<&RegionChunk>) -> u8 {
    match chunk.map(|chunk| &chunk.data) {
        Some(ChunkData::Raw { compression, .. })
        | Some(ChunkData::External { compression })
        | Some(ChunkData::Nbt { compression, .. }) => *compression & 0x7F,
        None => 2,
    }
}

fn apply_changes(
    mut files: Files,
    region: Coords,
    set: ChunkSet,
    changes: &ChangeSet,
    force: bool,
) -> Changes {
    let needs = changes.needs();
    let mut touched = [false; 3];
    let mut changed_chunks = 0;
    let mut errors = Vec::new();
    let before: Vec<BTreeSet<usize>> = KINDS
        .iter()
        .map(|kind| {
            files
                .get(kind)
                .map(|l| externals(&l.region))
                .unwrap_or_default()
        })
        .collect();
    for index in set.iter() {
        if !files.present(index) {
            continue;
        }
        let decode = |loaded: Option<&Loaded>, wanted: bool| match loaded {
            Some(loaded) if wanted => loaded.decode(index),
            _ => Ok(None),
        };
        let roots = (
            decode(files.region.as_ref(), needs.region),
            decode(files.entities.as_ref(), needs.entities),
            decode(files.poi.as_ref(), needs.poi),
        );
        let (mut region_root, mut entities_root, mut poi_root) = match roots {
            (Ok(r), Ok(e), Ok(p)) => (r, e, p),
            (Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => {
                errors.push(format!("chunk {index}: {err}"));
                continue;
            }
        };
        let stored = files.region.as_ref().and_then(|l| l.region.get(index));
        let old_timestamp = stored.map_or(0, |chunk| chunk.timestamp);
        let old_compression = compression_of(stored);
        let mut timestamp = old_timestamp;
        let mut compression = old_compression;
        let (x, z) = chunk_at(region, index);
        let changed = changes.apply(
            &mut ChunkMut {
                x,
                z,
                region: region_root.as_mut(),
                entities: entities_root.as_mut(),
                poi: poi_root.as_mut(),
                timestamp: &mut timestamp,
                compression: &mut compression,
            },
            force,
        );
        let mut any = false;
        if let Some(loaded) = files.region.as_mut()
            && let Some(chunk) = loaded.region.get_mut(index)
        {
            if changed.region || compression != old_compression {
                match region_root.take() {
                    Some(root) => {
                        chunk.data = ChunkData::Nbt { compression, root };
                        any = true;
                    }
                    None => errors.push(format!("chunk {index}: the chunk data was not read")),
                }
            }
            if timestamp != old_timestamp {
                chunk.timestamp = timestamp;
                any = true;
            }
            touched[0] |= any;
        }
        for (slot, loaded, root, flag) in [
            (
                1,
                files.entities.as_mut(),
                entities_root.take(),
                changed.entities,
            ),
            (2, files.poi.as_mut(), poi_root.take(), changed.poi),
        ] {
            if !flag {
                continue;
            }
            if let (Some(loaded), Some(root)) = (loaded, root)
                && loaded.region.get(index).is_some()
            {
                loaded.region.replace(index, root);
                touched[slot] = true;
                any = true;
            }
        }
        if any {
            changed_chunks += 1;
        }
    }
    let mut out = Vec::new();
    for (i, (kind, loaded)) in [
        ("region", files.region.take()),
        ("entities", files.entities.take()),
        ("poi", files.poi.take()),
    ]
    .into_iter()
    .enumerate()
    {
        if touched[i]
            && let Some(loaded) = loaded
        {
            out.push((kind, loaded, before[i].clone()));
        }
    }
    (out, changed_chunks, errors)
}

impl<'a> AsyncStep<'a> for ChangeStep<'a> {
    fn folder(&self) -> &str {
        &self.folder
    }

    fn run<'s>(
        &'s mut self,
        writer: &'s mut Writer<'a>,
        region: Coords,
        tally: &'s mut Tally,
    ) -> StepFuture<'s> {
        Box::pin(async move {
            let Some(set) = self.resolved.get(&region).copied() else {
                return Ok(());
            };
            let _slot = tiles::slot(self.map.settings.render_concurrency)
                .await
                .map_err(|_| Stop::from("the panel is shutting down"))?;
            let needs = self.changes.needs();
            let files = Files::load(
                self.map,
                &self.folder,
                &self.listing,
                region,
                needs,
                Some(&set),
            )
            .await?;
            let changes = self.changes.clone();
            let force = self.force;
            let (outputs, changed, errors) = tokio::task::spawn_blocking(move || {
                apply_changes(files, region, set, &changes, force)
            })
            .await
            .map_err(blocking_failed)?;
            tally.chunks += changed;
            for error in errors {
                note(
                    &mut tally.errors,
                    &mut tally.failures,
                    &region_label(region),
                    error,
                );
            }
            for (kind, loaded, external_before) in outputs {
                tally.files += write_output(
                    writer,
                    &self.folder,
                    region,
                    Output {
                        path: loaded.path,
                        kind,
                        region: loaded.region,
                        external_before,
                    },
                )
                .await?;
            }
            Ok(())
        })
    }
}

/// Changes NBT fields of the selected chunks (MCA Selector's "Change fields").
pub async fn change(
    handle: &Handle,
    map: &Map,
    user: Uuid,
    place: (&str, &str),
    folder: &str,
    selection: &Selection,
    changes: Arc<ChangeSet>,
    force: bool,
) -> Result<serde_json::Value, Stop> {
    let listing = Listing::read(map, folder).await?;
    let resolved = resolve_limited(
        selection,
        &listing.all(),
        u64::from(map.settings.max_delete_chunks),
    )?;
    let selected: u64 = resolved.values().map(|set| u64::from(set.count())).sum();
    let regions: Vec<Coords> = resolved.keys().copied().collect();
    let summary = json!({ "selected": selected, "changes": changes.to_query(), "force": force });
    let step = ChangeStep {
        map,
        folder: folder.to_string(),
        listing,
        resolved,
        changes,
        force,
    };
    write_job(
        handle,
        map,
        user,
        OperationKind::Change,
        place,
        regions,
        summary,
        step,
    )
    .await
}

/// What an import copies and how.
pub struct ImportPlan {
    pub source_folder: String,
    pub target_folder: String,
    pub offset: Offset,
    pub overwrite: bool,
    pub source_selection: Option<Selection>,
    pub target_selection: Option<Selection>,
    pub ranges: Ranges,
}

/// MCA Selector's `createTargetSourceMapping`: target region -> the source regions (1, 2 or 4)
/// its chunks come from.
pub fn target_mapping(
    sources: &BTreeSet<Coords>,
    offset: Offset,
    source_selection: Option<&Selection>,
    target_selection: Option<&Selection>,
) -> BTreeMap<Coords, BTreeSet<Coords>> {
    let mut mapping: BTreeMap<Coords, BTreeSet<Coords>> = BTreeMap::new();
    for &source in sources {
        if source_selection.is_some_and(|s| !any_in_region(s, source)) {
            continue;
        }
        let (cx, cz) = (source.0 * 32 + offset.x, source.1 * 32 + offset.z);
        for (dx, dz) in [(0, 0), (31, 0), (0, 31), (31, 31)] {
            let target = (
                coords::region_of_chunk(cx + dx),
                coords::region_of_chunk(cz + dz),
            );
            if target_selection.is_none_or(|s| any_in_region(s, target)) {
                mapping.entry(target).or_default().insert(source);
            }
        }
    }
    mapping
}

/// The source slots of `source` region whose chunks land in `target` region.
fn landing(source: Coords, target: Coords, offset: Offset) -> ChunkSet {
    let mut set = ChunkSet::default();
    for index in 0..anvil::CHUNKS {
        let (x, z) = chunk_at(source, index);
        let (tx, tz) = (x + offset.x, z + offset.z);
        if (coords::region_of_chunk(tx), coords::region_of_chunk(tz)) == target {
            set.set(index);
        }
    }
    set
}

/// Merges the source chunks of one kind into the target region in memory (MCA Selector's
/// `mergeChunksInto`); the number of chunks imported and skipped, and errors.
#[allow(clippy::too_many_arguments)]
fn merge_kind(
    kind: &str,
    target: &mut Region,
    target_external: &HashMap<usize, Vec<u8>>,
    target_coords: Coords,
    sources: &[(Coords, Loaded)],
    plan: &ImportPlan,
) -> (u64, u64, Vec<String>, bool) {
    let (mut imported, mut skipped, mut errors, mut changed) = (0, 0, Vec::new(), false);
    let offset = plan.offset;
    let moved = offset != Offset::default();
    for (source_coords, source) in sources {
        for index in source.region.occupied() {
            let (sx, sz) = chunk_at(*source_coords, index);
            let (tx, tz) = (sx + offset.x, sz + offset.z);
            if (coords::region_of_chunk(tx), coords::region_of_chunk(tz)) != target_coords {
                continue;
            }
            if plan
                .source_selection
                .as_ref()
                .is_some_and(|s| !s.is_selected(sx, sz))
                || plan
                    .target_selection
                    .as_ref()
                    .is_some_and(|s| !s.is_selected(tx, tz))
            {
                continue;
            }
            let target_index = coords::slot(tx, tz);
            if !plan.overwrite && target.get(target_index).is_some() {
                skipped += 1;
                continue;
            }
            let stored = source.region.get(index).expect("occupied slot");
            // a plain copy keeps the chunk's bytes; entities always go through relocation,
            // which gives them new UUIDs as MCA Selector does
            if kind == "region"
                && !moved
                && plan.ranges.is_all()
                && let ChunkData::Raw { .. } = stored.data
            {
                target.set(target_index, Some(stored.clone()));
                imported += 1;
                changed = true;
                continue;
            }
            let result = (|| -> Result<RegionChunk, String> {
                let mut root = source.decode(index)?.ok_or(
                    "the chunk is stored outside the region file but its .mcc file is missing",
                )?;
                match kind {
                    "region" => relocate::relocate_region(&mut root, offset)?,
                    "entities" => relocate::relocate_entities(&mut root, offset)?,
                    _ => relocate::relocate_poi(&mut root, offset)?,
                }
                let compression = compression_of(Some(stored));
                if plan.ranges.is_all() {
                    if kind == "region" && offset.y != 0 {
                        changer::fix_heightmaps(&mut root);
                    }
                    return Ok(RegionChunk {
                        timestamp: stored.timestamp,
                        data: ChunkData::Nbt { compression, root },
                    });
                }
                let version = root.i64("DataVersion").unwrap_or(0) as i32;
                if version == 0 {
                    return Err("the chunk has no DataVersion".into());
                }
                let existing = target.decode(
                    target_index,
                    target_external.get(&target_index).map(Vec::as_slice),
                )?;
                let (mut destination, destination_compression) = match existing {
                    Some(destination) => (destination, compression_of(target.get(target_index))),
                    None => (
                        match kind {
                            "region" => relocate::empty_region(tx, tz, version),
                            "entities" => relocate::empty_entities(tx, tz, version),
                            _ => relocate::empty_poi(tx, tz, version),
                        },
                        2,
                    ),
                };
                match kind {
                    "region" => {
                        relocate::merge_region(&root, &mut destination, &plan.ranges, offset.y)?;
                        changer::fix_heightmaps(&mut destination);
                    }
                    "entities" => {
                        relocate::merge_entities(&root, &mut destination, &plan.ranges, offset.y)?
                    }
                    _ => relocate::merge_poi(&root, &mut destination, &plan.ranges, offset.y)?,
                }
                Ok(RegionChunk {
                    timestamp: stored.timestamp,
                    data: ChunkData::Nbt {
                        compression: destination_compression,
                        root: destination,
                    },
                })
            })();
            match result {
                Ok(chunk) => {
                    target.set(target_index, Some(chunk));
                    imported += 1;
                    changed = true;
                }
                Err(err) => errors.push(format!("chunk {sx}, {sz}: {err}")),
            }
        }
    }
    (imported, skipped, errors, changed)
}

struct ImportStep<'a> {
    map: &'a Map,
    plan: Arc<ImportPlan>,
    source_listing: Listing,
    target_listing: Listing,
    mapping: BTreeMap<Coords, BTreeSet<Coords>>,
}

impl<'a> AsyncStep<'a> for ImportStep<'a> {
    fn folder(&self) -> &str {
        &self.plan.target_folder
    }

    fn run<'s>(
        &'s mut self,
        writer: &'s mut Writer<'a>,
        target: Coords,
        tally: &'s mut Tally,
    ) -> StepFuture<'s> {
        Box::pin(async move {
            let Some(source_regions) = self.mapping.get(&target).cloned() else {
                return Ok(());
            };
            let _slot = tiles::slot(self.map.settings.render_concurrency)
                .await
                .map_err(|_| Stop::from("the panel is shutting down"))?;
            for kind in KINDS {
                let mut sources = Vec::new();
                for &source in &source_regions {
                    if !self.source_listing.has(kind, source) {
                        continue;
                    }
                    let wanted = landing(source, target, self.plan.offset);
                    if let Some(loaded) = load(
                        self.map,
                        &self.plan.source_folder,
                        kind,
                        source,
                        Some(&wanted),
                    )
                    .await?
                    {
                        sources.push((source, loaded));
                    }
                }
                if sources.is_empty() {
                    continue;
                }
                let target_loaded = if self.target_listing.has(kind, target) {
                    load(
                        self.map,
                        &self.plan.target_folder,
                        kind,
                        target,
                        Some(&ChunkSet::full()),
                    )
                    .await?
                } else {
                    None
                };
                let (path, mut region, external) = match target_loaded {
                    Some(loaded) => (loaded.path, loaded.region, loaded.external),
                    None => (
                        file_path(&self.plan.target_folder, kind, target),
                        Region::empty(),
                        HashMap::new(),
                    ),
                };
                let external_before = externals(&region);
                let plan = self.plan.clone();
                let (region, imported, skipped, errors, changed) =
                    tokio::task::spawn_blocking(move || {
                        let (imported, skipped, errors, changed) =
                            merge_kind(kind, &mut region, &external, target, &sources, &plan);
                        (region, imported, skipped, errors, changed)
                    })
                    .await
                    .map_err(blocking_failed)?;
                if kind == "region" {
                    tally.chunks += imported;
                    tally.skipped += skipped;
                }
                for error in errors {
                    note(
                        &mut tally.errors,
                        &mut tally.failures,
                        &region_label(target),
                        error,
                    );
                }
                if !changed {
                    continue;
                }
                tally.files += write_output(
                    writer,
                    &self.plan.target_folder,
                    target,
                    Output {
                        path,
                        kind,
                        region,
                        external_before,
                    },
                )
                .await?;
            }
            Ok(())
        })
    }
}

/// Copies chunks from a source dimension or folder into the target dimension with an offset
/// (MCA Selector's "Import chunks"). When source and target are the same folder, the source
/// files are first copied into a staging folder so chunks written early are not read again.
pub async fn import(
    handle: &Handle,
    map: &Map,
    user: Uuid,
    place: (&str, &str),
    mut plan: ImportPlan,
    staging: &str,
) -> Result<serde_json::Value, Stop> {
    let target_listing = Listing::read(map, &plan.target_folder).await?;
    let mut source_listing = Listing::read(map, &plan.source_folder).await?;
    let sources = source_listing.all();
    if sources.is_empty() {
        return Err(Stop::from("the source has no region files"));
    }
    let mapping = target_mapping(
        &sources,
        plan.offset,
        plan.source_selection.as_ref(),
        plan.target_selection.as_ref(),
    );
    if mapping.is_empty() {
        return Err(Stop::from("no source chunk lands in the target selection"));
    }
    // a bound on the work: every target region could be filled
    let most = mapping.len() as u64 * anvil::CHUNKS as u64;
    let selected = plan
        .source_selection
        .as_ref()
        .map(|s| {
            s.resolve(&sources)
                .values()
                .map(|set| u64::from(set.count()))
                .sum::<u64>()
        })
        .unwrap_or(most);
    let limit = u64::from(map.settings.max_delete_chunks);
    if selected.min(most) > limit {
        return Err(Stop::Failed(format!(
            "the import may touch up to {} chunks; one operation may touch at most {limit} (admin setting): select fewer source chunks",
            selected.min(most)
        )));
    }
    let mut snapshot = None;
    if plan.source_folder == plan.target_folder {
        handle.message("copying the source region files first");
        let needed: BTreeSet<Coords> = mapping.values().flatten().copied().collect();
        for kind in KINDS {
            for &region in &needed {
                if !source_listing.has(kind, region) {
                    continue;
                }
                handle.check()?;
                let from = file_path(&plan.source_folder, kind, region);
                let to = file_path(staging, kind, region);
                if let Err(err) = map.wings.copy(&format!("/{from}"), &format!("/{to}")).await {
                    return Err(failed(err).await);
                }
            }
        }
        snapshot = Some(staging.to_string());
        plan.source_folder = staging.to_string();
        source_listing = Listing::read(map, staging).await?;
        handle.message("importing");
    }
    let regions: Vec<Coords> = mapping.keys().copied().collect();
    let summary = json!({
        "offset": plan.offset,
        "overwrite": plan.overwrite,
        "sections": if plan.ranges.is_all() { None } else { Some(true) },
        "source_regions": sources.len(),
    });
    let step = ImportStep {
        map,
        plan: Arc::new(plan),
        source_listing,
        target_listing,
        mapping,
    };
    let result = write_job(
        handle,
        map,
        user,
        OperationKind::Import,
        place,
        regions,
        summary,
        step,
    )
    .await;
    if let Some(folder) = snapshot
        && let Err(err) = map.wings.delete("/", &[folder]).await
    {
        tracing::warn!(
            "world map: could not remove the import snapshot: {}",
            jobs::describe(err).await
        );
    }
    result
}

/// A zip archive written as a stream: stored entries (region files are compressed already),
/// CRC-32 and sizes known before each entry, no ZIP64 (callers keep under 4 GiB and 65535
/// entries).
pub struct ZipWriter {
    offset: u64,
    entries: Vec<(String, u32, u32, u64)>,
}

impl ZipWriter {
    pub const MAX_ENTRIES: usize = 65_535;
    pub const MAX_BYTES: u64 = u32::MAX as u64 - (64 << 20);

    pub fn new() -> Self {
        Self {
            offset: 0,
            entries: Vec::new(),
        }
    }

    /// The bytes of a local entry for `name` with `data`.
    pub fn entry(&mut self, name: &str, data: &[u8]) -> Result<Vec<u8>, String> {
        if self.entries.len() >= Self::MAX_ENTRIES {
            return Err("too many files for one zip".into());
        }
        let size = u32::try_from(data.len()).map_err(|_| "a file is too large for a zip")?;
        if self.offset + data.len() as u64 > Self::MAX_BYTES {
            return Err("the export is larger than 4 GiB: select fewer chunks".into());
        }
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let crc = crc.sum();
        let mut out = Vec::with_capacity(30 + name.len() + data.len());
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend(20u16.to_le_bytes()); // version needed
        out.extend(0x0800u16.to_le_bytes()); // UTF-8 names
        out.extend(0u16.to_le_bytes()); // stored
        out.extend(0u16.to_le_bytes()); // time
        out.extend(0x0021u16.to_le_bytes()); // date 1980-01-01
        out.extend(crc.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend(size.to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name.as_bytes());
        out.extend_from_slice(data);
        self.entries
            .push((name.to_string(), crc, size, self.offset));
        self.offset += out.len() as u64;
        Ok(out)
    }

    /// The central directory and its end record.
    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, crc, size, offset) in &self.entries {
            out.extend(0x0201_4b50u32.to_le_bytes());
            out.extend(20u16.to_le_bytes()); // made by
            out.extend(20u16.to_le_bytes()); // needed
            out.extend(0x0800u16.to_le_bytes());
            out.extend(0u16.to_le_bytes());
            out.extend(0u16.to_le_bytes());
            out.extend(0x0021u16.to_le_bytes());
            out.extend(crc.to_le_bytes());
            out.extend(size.to_le_bytes());
            out.extend(size.to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend([0u8; 12]); // extra, comment, disk, internal attributes
            out.extend(0u32.to_le_bytes()); // external attributes
            out.extend((*offset as u32).to_le_bytes());
            out.extend(name.as_bytes());
        }
        let count = self.entries.len() as u16;
        out.extend(0x0605_4b50u32.to_le_bytes());
        out.extend([0u8; 4]);
        out.extend(count.to_le_bytes());
        out.extend(count.to_le_bytes());
        out.extend(0u32.to_le_bytes()); // central directory size, set below
        out.extend((self.offset as u32).to_le_bytes());
        out.extend(0u16.to_le_bytes());
        // the central directory size is everything before the end record
        let directory = (out.len() - 22) as u32;
        let at = out.len() - 10;
        out[at..at + 4].copy_from_slice(&directory.to_le_bytes());
        out
    }
}

/// The region file of `loaded` reduced to the chunks in `keep`, and the `.mcc` files of its
/// oversized chunks (gzip ones compressed again: Wings serves them decompressed).
fn export_region(
    loaded: &Loaded,
    keep: &ChunkSet,
) -> Result<Option<(Vec<u8>, Vec<(usize, Vec<u8>)>)>, String> {
    let mut region = Region::empty();
    let mut external = Vec::new();
    for index in loaded.region.occupied() {
        if !keep.get(index) {
            continue;
        }
        let chunk = loaded.region.get(index).expect("occupied slot").clone();
        if let ChunkData::External { compression } = chunk.data {
            let Some(bytes) = loaded.external.get(&index) else {
                continue;
            };
            let bytes = if compression & 0x7F == 1 && !bytes.starts_with(&[0x1F, 0x8B]) {
                nbt::gzip(bytes)
            } else {
                bytes.clone()
            };
            external.push((index, bytes));
        }
        region.set(index, Some(chunk));
    }
    if region.is_empty() {
        return Ok(None);
    }
    Ok(Some((region.write()?.region, external)))
}

/// Streams the selected chunks as a zip of `region/`, `entities/` and `poi/` folders (MCA
/// Selector's "Export selected chunks"), one region at a time.
pub async fn export(
    map: &Map,
    folder: &str,
    resolved: BTreeMap<Coords, ChunkSet>,
    listing: &Listing,
    mut sink: tokio::io::DuplexStream,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut zip = ZipWriter::new();
    for (region, keep) in resolved {
        let _slot = tiles::slot(map.settings.render_concurrency)
            .await
            .map_err(|_| "the panel is shutting down".to_string())?;
        for kind in KINDS {
            if !listing.has(kind, region) {
                continue;
            }
            let loaded = match load(map, folder, kind, region, Some(&keep)).await {
                Ok(Some(loaded)) => loaded,
                Ok(None) => continue,
                Err(Stop::Failed(err)) => return Err(err),
                Err(Stop::Cancelled) => return Err("cancelled".into()),
            };
            let keep_set = keep;
            let built = tokio::task::spawn_blocking(move || export_region(&loaded, &keep_set))
                .await
                .map_err(|_| "exporting a region failed".to_string())??;
            let Some((data, external)) = built else {
                continue;
            };
            for (index, bytes) in external {
                let (x, z) = chunk_at(region, index);
                let entry = zip.entry(&format!("{kind}/{}", anvil::mcc_name(x, z)), &bytes)?;
                sink.write_all(&entry)
                    .await
                    .map_err(|err| err.to_string())?;
            }
            let entry = zip.entry(
                &format!("{kind}/{}", coords::region_file_name(region.0, region.1)),
                &data,
            )?;
            sink.write_all(&entry)
                .await
                .map_err(|err| err.to_string())?;
        }
    }
    sink.write_all(&zip.finish())
        .await
        .map_err(|err| err.to_string())?;
    sink.shutdown().await.map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(inverted: bool, regions: &[(Coords, Option<&[usize]>)]) -> Selection {
        Selection {
            inverted,
            regions: regions
                .iter()
                .map(|(region, chunks)| {
                    (
                        *region,
                        chunks.map(|chunks| {
                            let mut set = ChunkSet::default();
                            for &i in chunks {
                                set.set(i);
                            }
                            set
                        }),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn region_selection_queries_follow_mca_selector() {
        let plain = selection(false, &[((0, 0), None), ((1, 0), Some(&[5]))]);
        assert!(any_in_region(&plain, (0, 0)));
        assert!(any_in_region(&plain, (1, 0)));
        assert!(!any_in_region(&plain, (2, 0)));
        assert_eq!(chunks_in_region(&plain, (0, 0)), Some(ChunkSet::full()));
        assert_eq!(chunks_in_region(&plain, (1, 0)).unwrap().count(), 1);
        assert_eq!(chunks_in_region(&plain, (2, 0)), None);

        let inverted = selection(true, &[((0, 0), None), ((1, 0), Some(&[5]))]);
        assert!(!any_in_region(&inverted, (0, 0)));
        assert!(any_in_region(&inverted, (1, 0)));
        assert!(any_in_region(&inverted, (7, 7)));
        assert_eq!(chunks_in_region(&inverted, (0, 0)), None);
        let rest = chunks_in_region(&inverted, (1, 0)).unwrap();
        assert_eq!(rest.count(), 1023);
        assert!(!rest.get(5));
        assert_eq!(chunks_in_region(&inverted, (7, 7)), Some(ChunkSet::full()));
    }

    #[test]
    fn targets_map_to_their_source_regions() {
        let sources = BTreeSet::from([(0, 0), (1, 0)]);
        // a region-aligned offset maps one to one
        let aligned = target_mapping(
            &sources,
            Offset {
                x: 64,
                y: 0,
                z: -32,
            },
            None,
            None,
        );
        assert_eq!(
            aligned,
            BTreeMap::from([
                ((2, -1), BTreeSet::from([(0, 0)])),
                ((3, -1), BTreeSet::from([(1, 0)])),
            ])
        );
        // an unaligned one spreads each source over four targets
        let spread = target_mapping(
            &BTreeSet::from([(0, 0)]),
            Offset { x: 33, y: 0, z: -2 },
            None,
            None,
        );
        assert_eq!(
            spread.keys().copied().collect::<Vec<_>>(),
            vec![(1, -1), (1, 0), (2, -1), (2, 0)]
        );
        // selections prune sources and targets
        let only = selection(false, &[((1, 0), None)]);
        let pruned = target_mapping(&sources, Offset::default(), Some(&only), None);
        assert_eq!(pruned.keys().copied().collect::<Vec<_>>(), vec![(1, 0)]);
        let target = selection(false, &[((0, 0), Some(&[0]))]);
        let into = target_mapping(&sources, Offset::default(), None, Some(&target));
        assert_eq!(into.keys().copied().collect::<Vec<_>>(), vec![(0, 0)]);

        let land = landing((0, 0), (1, 0), Offset { x: 1, y: 0, z: 0 });
        assert_eq!(land.count(), 32);
        assert!(land.get(31));
        assert!(!land.get(30));
    }

    #[test]
    fn zips_are_readable() {
        let mut zip = ZipWriter::new();
        let mut bytes = zip.entry("region/r.0.0.mca", b"hello").unwrap();
        bytes.extend(zip.entry("poi/r.0.0.mca", &[1, 2, 3]).unwrap());
        let local_size = bytes.len();
        bytes.extend(zip.finish());
        // end of central directory: 2 entries, directory right after the local entries
        let end = &bytes[bytes.len() - 22..];
        assert_eq!(&end[..4], &0x0605_4b50u32.to_le_bytes());
        assert_eq!(u16::from_le_bytes([end[10], end[11]]), 2);
        let directory_size = u32::from_le_bytes(end[12..16].try_into().unwrap()) as usize;
        let directory_offset = u32::from_le_bytes(end[16..20].try_into().unwrap()) as usize;
        assert_eq!(directory_offset, local_size);
        assert_eq!(directory_offset + directory_size, bytes.len() - 22);
        assert_eq!(
            &bytes[directory_offset..directory_offset + 4],
            &0x0201_4b50u32.to_le_bytes()
        );
        // the first local entry holds the data stored, with its CRC-32
        let mut crc = flate2::Crc::new();
        crc.update(b"hello");
        assert_eq!(
            u32::from_le_bytes(bytes[14..18].try_into().unwrap()),
            crc.sum()
        );
        assert_eq!(&bytes[30 + 16..30 + 16 + 5], b"hello");
    }
}
