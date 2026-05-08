## 0.2.0 (2026-05-08)

### Breaking Changes

- `fs::extract_archive` now consumes its `File` by value
(was `&mut File`), and `WasmEdgeApiClient::releases`, `latest_release`,
and `resolve_version` are now `async fn` (were sync). Both shifts are
required by the spawn_blocking change — backward-compat wrappers would
either be subtly buggy (`try_clone()` on the File would share the seek
position, so the blocking task's `rewind()` would silently mutate the
caller's `&mut File`) or footgunny (sync wrappers around async client
methods would either panic inside an existing tokio runtime or spin up
a fresh nested one). The crate is published primarily as a CLI tool;
the lib API exists to share code with the binary and integration tests
and is at 0.1.x where SemVer permits breakage between minor versions.
The next release should bump the minor version accordingly.

### Features

- verify SHA256 checksum on plugin install (#268)

### Fixes

- resolve 'latest' against locally installed versions (#266)
- surface partial-install failures from copy_tree (#271)

## 0.1.3 (2026-02-11)

### Features

- validate code formatting, build, and tests on PR & push (#3)
- setup cli structure (#11)
- add CI integration with Knope (#12)
- change knope to use GitHub bot (#21)
- command 'wasmedgeup list' (#17)
- implement `wasmedgeup list` with libgit2 (#25)
- install WasmEdge to specified directory (#26)
- add WasmEdge to user's PATH after installation (rustup-like) (#52)
- add ci clippy check and fix warnings (#97)
- add checksum verification and install fixes (#96)
- implement version management using use and list  (#100)
- implement remove command with tests (#104)
- detect system specs and plugin list (#113)
- add plugin install command (#132)
- implement plugin remove command (#138)
- add --no-verify to skip checksum verification (#157)

### Fixes

- workflows file formatting (#13)
- knope action version (#16)
- update `mozilla-actions/sccache-action` to v0.0.8 (#20)
- knope bot commit message (#30)
- resolve Windows CI build errors and warnings (#145)

## 0.1.2 (2026-02-10)

### Features

- validate code formatting, build, and tests on PR & push (#3)
- setup cli structure (#11)
- add CI integration with Knope (#12)
- change knope to use GitHub bot (#21)
- command 'wasmedgeup list' (#17)
- implement `wasmedgeup list` with libgit2 (#25)
- install WasmEdge to specified directory (#26)
- add WasmEdge to user's PATH after installation (rustup-like) (#52)
- add ci clippy check and fix warnings (#97)
- add checksum verification and install fixes (#96)
- implement version management using use and list  (#100)
- implement remove command with tests (#104)
- detect system specs and plugin list (#113)
- add plugin install command (#132)
- implement plugin remove command (#138)
- add --no-verify to skip checksum verification (#157)

### Fixes

- workflows file formatting (#13)
- knope action version (#16)
- update `mozilla-actions/sccache-action` to v0.0.8 (#20)
- knope bot commit message (#30)
- resolve Windows CI build errors and warnings (#145)

## 0.1.1 (2025-05-05)

### Features

- install WasmEdge to specified directory (#26)
- implement `wasmedgeup list` with libgit2 (#25)
- command 'wasmedgeup list' (#17)
- change knope to use GitHub bot (#21)
- add CI integration with Knope (#12)
- setup cli structure (#11)
- validate code formatting, build, and tests on PR & push (#3)
- add `wasmedgeup list` command

### Fixes

- knope bot commit message (#30)
- update `mozilla-actions/sccache-action` to v0.0.8 (#20)
- knope action version (#16)
- workflows file formatting (#13)
