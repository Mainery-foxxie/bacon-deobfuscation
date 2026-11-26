# Bacon Guard deobfuscator — Windows x64

Extract the ZIP and open PowerShell in the folder containing `bgdeobf.exe`. The bundled tools run without installing Rust or Python.

```powershell
.\bgdeobf.exe --check
.\bgdeobf.exe .\test\samples\safe_vm.lua -o .\example.deobf.lua
```

`--check` lists the three Luau tools in `lua_bin/`. The self-authored sample decompiles to a script that prints `safe fixture` and `41`, then returns `41`. For your own script, replace the input and output paths. Run `.\bgdeobf.exe --help` for other options.

## Discord bot

Copy `.env.example` to `.env`, set `DISCORD_TOKEN` in that file, then run `.\bgdeobf-bot.exe`. Keep it beside `bgdeobf.exe`, which the bot uses as its worker. The other bot settings are documented in `.env.example`.

## Build and test

`src/` contains the Rust code and embedded Lua templates. `test/` contains the integration tests and fixtures. From the folder containing `Cargo.toml`, with the Windows MSVC Rust toolchain:

```powershell
cargo build --release --locked --bins --example recover_baconboss
cargo test --locked --all-targets -- --test-threads=1
```

## BaconBoss verification

`recover_baconboss.exe` includes `compare-saved` for checking saved compiler output offline. In the recorded sample, 216 of 222 functions matched exactly and two matched after reordering. Four (`177`, `216`, `220`, `221`) remain unverified as whole functions. One region inside `216` matched for 65,536 modeled outcomes; that does not establish a match for the whole function. Protection checks were retained and the application was not run. See [the verification report](test/verification/artifact-verification.json) for the evidence and recorded Rust test results.

## GitHub release

Attach `sssss.zip` and `sssss.zip.sha256` to the release. Compare `Get-FileHash .\sssss.zip -Algorithm SHA256` with the checksum before extracting.

The project uses [Apache-2.0](LICENSE). The Luau tools have a [separate MIT license](lua_bin/LICENSE-luau.txt); Rust dependency notices are in [THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt).
