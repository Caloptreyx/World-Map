//! Entities from the `entities/r.<x>.<z>.mca` region files (1.17+): each chunk holds an
//! `Entities` list with the entity's `id`, `Pos` and optional `CustomName`.
use crate::{
    anvil,
    nbt::{Compound, Tag},
};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entity {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The plain text of a custom name: a JSON text component (before 1.21.5), a text component
/// compound (since) or a plain string.
fn custom_name(entity: &Compound) -> Option<String> {
    let text = match entity.get("CustomName")? {
        Tag::String(value) => match serde_json::from_str::<serde_json::Value>(value) {
            Ok(serde_json::Value::String(text)) => text,
            Ok(serde_json::Value::Object(component)) => component
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => value.clone(),
        },
        Tag::Compound(component) => component.str("text").unwrap_or_default().to_string(),
        _ => return None,
    };
    let text: String = text.chars().filter(|c| !c.is_control()).take(64).collect();
    (!text.is_empty()).then_some(text)
}

fn entity(tag: &Tag) -> Option<Entity> {
    let entity = tag.as_compound()?;
    let pos = entity.numbers("Pos").filter(|pos| pos.len() == 3)?;
    if !pos.iter().all(|value| value.is_finite()) {
        return None;
    }
    Some(Entity {
        id: entity.str("id")?.to_string(),
        x: pos[0],
        y: pos[1],
        z: pos[2],
        name: custom_name(entity),
    })
}

/// Every entity stored in an entity region file; unreadable chunks are skipped.
pub fn region(data: &[u8]) -> Result<Vec<Entity>, String> {
    let mut entities = Vec::new();
    for slot in anvil::slots(data)? {
        let Ok(Some(chunk)) = anvil::chunk(data, slot) else {
            continue;
        };
        entities.extend(
            chunk
                .list("Entities")
                .unwrap_or_default()
                .iter()
                .filter_map(entity),
        );
    }
    Ok(entities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const ENTITIES: &[u8] = include_bytes!("../tests/fixtures/entities.mca");

    #[test]
    fn reads_the_recorded_entities() {
        let expected: Value =
            serde_json::from_str(include_str!("../tests/fixtures/expected.json")).unwrap();
        let entities = region(ENTITIES).unwrap();
        let expected = expected["entities"].as_array().unwrap();
        assert_eq!(entities.len(), expected.len());
        assert!(!entities.is_empty());
        for (entity, want) in entities.iter().zip(expected) {
            assert_eq!(entity.id, want[0].as_str().unwrap());
            let coordinates = &want.as_array().unwrap()[1..4];
            for (got, want) in [entity.x, entity.y, entity.z].into_iter().zip(coordinates) {
                assert!((got - want.as_f64().unwrap()).abs() < 1e-9, "{entity:?}");
            }
        }
    }

    #[test]
    fn custom_names_of_every_era() {
        let named = |tag: Tag| {
            let mut data = vec![10, 0, 0, 8, 0, 2, b'i', b'd', 0, 3, b'p', b'i', b'g'];
            match tag {
                Tag::String(value) => {
                    data.extend([8, 0, 10]);
                    data.extend(b"CustomName");
                    data.extend((value.len() as u16).to_be_bytes());
                    data.extend(value.as_bytes());
                }
                _ => {
                    data.extend([10, 0, 10]);
                    data.extend(b"CustomName");
                    data.extend([8, 0, 4]);
                    data.extend(b"text");
                    data.extend([0, 5]);
                    data.extend(b"Bacon");
                    data.push(0);
                }
            }
            data.push(0);
            custom_name(&crate::nbt::read(&data).unwrap())
        };
        assert_eq!(named(Tag::String(r#"{"text":"Bacon"}"#.into())), Some("Bacon".into()));
        assert_eq!(named(Tag::String(r#""Bacon""#.into())), Some("Bacon".into()));
        assert_eq!(named(Tag::String("Bacon".into())), Some("Bacon".into()));
        assert_eq!(named(Tag::Byte(0)), Some("Bacon".into()));
        assert_eq!(named(Tag::String(r#"{"text":""}"#.into())), None);
    }
}
