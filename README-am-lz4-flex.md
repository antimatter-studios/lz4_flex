# am-lz4-flex

A fork of [lz4_flex](https://github.com/PSeitz/lz4_flex) by Pascal Seitz and
its contributors, published only so that crates depending on the change below
can be released before upstream ships it. It is the code of
[PSeitz/lz4_flex#244](https://github.com/PSeitz/lz4_flex/pull/244) and nothing
else: lz4_flex 0.13's API, plus LZ4HC.

**Use [lz4_flex](https://crates.io/crates/lz4_flex) unless you need LZ4HC
today.** When upstream releases this change, this crate stops getting new
versions and its users move back to lz4_flex.

## What it adds to lz4_flex

- `compress_hc_to_vec`, `compress_hc_into` and `compress_hc_to_vec_with_table`:
  LZ4HC block compression at levels 1 to 12, decoded by any LZ4 decoder.
- `compress_hc_dest_size` and `compress_hc_dest_size_with_table`: as much of
  the input as fits in a fixed output, like liblz4's
  `LZ4_compress_HC_destSize`, for filesystems that compress into fixed-size
  clusters.

## Using it

The library is named `lz4_flex`, as upstream's is. Next to lz4_flex itself,
rename it:

```toml
[dependencies]
lz4_flex_hc = { package = "am-lz4-flex", version = "0.14" }
```

```rust
let compressed = lz4_flex_hc::block::compress_hc_to_vec(b"some input", 12);
```

## Licence

MIT, as lz4_flex. See [LICENSE](LICENSE).
