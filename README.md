# tfs-rust

A Rust port of [The Forgotten Server](https://github.com/otland/forgottenserver)
1.5, targeting the Tibia 8.60 protocol. It aims for behavioral parity with the
C++ server: same wire protocol, same file formats (`.otb`, `.otbm`), same Lua
API surface, same config keys, and the same MySQL schema. An unmodified 8.60
client and a stock `data/` folder are meant to work against it unchanged.

This work is based on [Nekiro's TFS 1.5 8.60 downgrade](https://github.com/nekiro/TFS-1.5-Downgrades),
itself a downgrade of The Forgotten Server by the OTLand community.

## Building

```
cargo build --release
```

The binary reads `config.lua` from the working directory and accepts
`--config`, `--ip`, `--login-port`, and `--game-port` flags.

## License

GNU General Public License v2.0 - see [LICENSE](LICENSE). This is a derivative
work of The Forgotten Server, which is also distributed under the GPLv2.
