"""Minimal Java Edition NBT and region file helpers for scripts/make_fixtures.py (standard library only)."""

import struct
import zlib

END, BYTE, SHORT, INT, LONG, FLOAT, DOUBLE, BYTE_ARRAY, STRING, LIST, COMPOUND, INT_ARRAY, LONG_ARRAY = range(13)


class Tagged:
    """A value with an explicit tag type, for writing."""

    def __init__(self, kind, value, item_kind=None):
        self.kind, self.value, self.item_kind = kind, value, item_kind


def read(data):
    """The root compound of uncompressed NBT as nested dicts/lists of Tagged values."""
    pos = 0

    def take(n):
        nonlocal pos
        out = data[pos : pos + n]
        pos += n
        return out

    def string():
        (n,) = struct.unpack(">H", take(2))
        return take(n).decode("utf-8", "replace")

    def payload(kind):
        if kind == BYTE:
            return Tagged(kind, struct.unpack(">b", take(1))[0])
        if kind == SHORT:
            return Tagged(kind, struct.unpack(">h", take(2))[0])
        if kind == INT:
            return Tagged(kind, struct.unpack(">i", take(4))[0])
        if kind == LONG:
            return Tagged(kind, struct.unpack(">q", take(8))[0])
        if kind == FLOAT:
            return Tagged(kind, struct.unpack(">f", take(4))[0])
        if kind == DOUBLE:
            return Tagged(kind, struct.unpack(">d", take(8))[0])
        if kind == BYTE_ARRAY:
            (n,) = struct.unpack(">i", take(4))
            return Tagged(kind, list(struct.unpack(f">{n}b", take(n))))
        if kind == STRING:
            return Tagged(kind, string())
        if kind == LIST:
            item, n = struct.unpack(">bi", take(5))
            return Tagged(kind, [payload(item) for _ in range(n)], item)
        if kind == COMPOUND:
            out = {}
            while True:
                (child,) = struct.unpack(">b", take(1))
                if child == END:
                    return Tagged(kind, out)
                name = string()
                out[name] = payload(child)
        if kind == INT_ARRAY:
            (n,) = struct.unpack(">i", take(4))
            return Tagged(kind, list(struct.unpack(f">{n}i", take(4 * n))))
        if kind == LONG_ARRAY:
            (n,) = struct.unpack(">i", take(4))
            return Tagged(kind, list(struct.unpack(f">{n}q", take(8 * n))))
        raise ValueError(f"bad tag {kind}")

    (kind,) = struct.unpack(">b", take(1))
    assert kind == COMPOUND
    string()
    return payload(COMPOUND)


def write(root):
    out = bytearray()

    def string(value):
        raw = value.encode("utf-8")
        out.extend(struct.pack(">H", len(raw)))
        out.extend(raw)

    def payload(tag):
        k, v = tag.kind, tag.value
        if k == BYTE:
            out.extend(struct.pack(">b", v))
        elif k == SHORT:
            out.extend(struct.pack(">h", v))
        elif k == INT:
            out.extend(struct.pack(">i", v))
        elif k == LONG:
            out.extend(struct.pack(">q", v))
        elif k == FLOAT:
            out.extend(struct.pack(">f", v))
        elif k == DOUBLE:
            out.extend(struct.pack(">d", v))
        elif k == BYTE_ARRAY:
            out.extend(struct.pack(f">i{len(v)}b", len(v), *v))
        elif k == STRING:
            string(v)
        elif k == LIST:
            out.extend(struct.pack(">bi", tag.item_kind if v else (tag.item_kind or END), len(v)))
            for item in v:
                payload(item)
        elif k == COMPOUND:
            for name, child in v.items():
                out.extend(struct.pack(">b", child.kind))
                string(name)
                payload(child)
            out.append(END)
        elif k == INT_ARRAY:
            out.extend(struct.pack(f">i{len(v)}i", len(v), *v))
        elif k == LONG_ARRAY:
            out.extend(struct.pack(f">i{len(v)}q", len(v), *v))

    out.append(COMPOUND)
    string("")
    payload(root)
    return bytes(out)


def plain(tag):
    """Tagged values as plain Python values, for inspection."""
    if tag.kind == COMPOUND:
        return {k: plain(v) for k, v in tag.value.items()}
    if tag.kind == LIST:
        return [plain(v) for v in tag.value]
    return tag.value


def region_chunks(data):
    """(index, compression, payload) of every chunk in a region file."""
    out = []
    for index in range(1024):
        (location,) = struct.unpack(">I", data[index * 4 : index * 4 + 4])
        if not location:
            continue
        sector = location >> 8
        length, compression = struct.unpack(">IB", data[sector * 4096 : sector * 4096 + 5])
        out.append((index, compression, data[sector * 4096 + 5 : sector * 4096 + 4 + length]))
    return out


def chunk_nbt(compression, payload):
    if compression == 2:
        return read(zlib.decompress(payload))
    if compression == 1:
        import gzip

        return read(gzip.decompress(payload))
    if compression == 3:
        return read(payload)
    raise ValueError(f"compression {compression}")


def build_region(chunks, timestamp=1_700_000_000):
    """A region file from (index, compression, payload) entries, packed in index order."""
    header = bytearray(8192)
    body = bytearray()
    sector = 2
    for index, compression, payload in sorted(chunks):
        blob = struct.pack(">IB", len(payload) + 1, compression) + payload
        blob += b"\0" * (-len(blob) % 4096)
        count = len(blob) // 4096
        struct.pack_into(">I", header, index * 4, (sector << 8) | count)
        struct.pack_into(">I", header, 4096 + index * 4, timestamp)
        body.extend(blob)
        sector += count
    return bytes(header + body)
