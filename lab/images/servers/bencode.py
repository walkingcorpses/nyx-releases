"""bencode.py - minimal bencode encode/decode for the leak lab's tracker, observer and torrent
generator. Only what BitTorrent metadata needs: int, bytes/str, list, dict (keys sorted on encode)."""


def encode(v):
    """Encodes a Python value (int, str, bytes, list, dict) as bencode bytes."""
    if isinstance(v, bool):
        raise TypeError("bool is not bencodable")
    if isinstance(v, int):
        return b"i%de" % v
    if isinstance(v, str):
        v = v.encode()
    if isinstance(v, bytes):
        return b"%d:%s" % (len(v), v)
    if isinstance(v, list):
        return b"l" + b"".join(encode(x) for x in v) + b"e"
    if isinstance(v, dict):
        items = sorted((k.encode() if isinstance(k, str) else k, x) for k, x in v.items())
        return b"d" + b"".join(encode(k) + encode(x) for k, x in items) + b"e"
    raise TypeError(f"cannot bencode {type(v).__name__}")


def decode(data):
    """Decodes one bencoded value from the start of `data`; returns (value, bytes consumed).
    Byte strings stay bytes; dict keys too."""
    return _dec(data, 0)


def _dec(d, i):
    """Recursive worker for decode: value starting at index i, and the index after it."""
    c = d[i:i + 1]
    if c == b"i":
        j = d.index(b"e", i)
        return int(d[i + 1:j]), j + 1
    if c == b"l":
        out, i = [], i + 1
        while d[i:i + 1] != b"e":
            v, i = _dec(d, i)
            out.append(v)
        return out, i + 1
    if c == b"d":
        out, i = {}, i + 1
        while d[i:i + 1] != b"e":
            k, i = _dec(d, i)
            v, i = _dec(d, i)
            out[k] = v
        return out, i + 1
    if c.isdigit():
        j = d.index(b":", i)
        n = int(d[i:j])
        return d[j + 1:j + 1 + n], j + 1 + n
    raise ValueError(f"bad bencode at {i}")


if __name__ == "__main__":
    # Self-check: round trip of a nested value.
    v = {"a": [1, b"x", {"k": "v"}], "b": -3}
    raw = encode(v)
    assert decode(raw) == ({b"a": [1, b"x", {b"k": b"v"}], b"b": -3}, len(raw)), decode(raw)
    print("bencode ok")
