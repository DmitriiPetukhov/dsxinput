<!-- Tracking explicitly requested by the user. -->
# Controller lifecycle fixes
Design waived explicitly by user. Plan each fix before implementation.
Branch: fix/controller-lifecycle; base: main. One PR scope, no PR creation.
Done: 3/3. Status: complete. Next: done. Optional brief/slice notes absent.

## 1. Bounded shutdown and cleanup on every runtime exit [behavioral]
Plan: isolate child ownership in a guard; TERM, timed grace, KILL without indefinite wait; drop device resources before child shutdown; rely on physical descriptor close to flush effects on all exits. Preserve normal child status. Tests: cooperative/stubborn child, error unwinding and reaping.
- [x] 🔴 impl: child shutdown regression tests
- [x] 🔴 ver: missing Game type; descendant test then reproduced leak
- [x] 🟢 impl: child guard, drop ordering, group escalation
- [x] 🟢 ver: 45 tests; clippy -D warnings; fmt available
- [x] 📦 commit: review clean after 2 fixes; hash 18ccbde

## 2. Initialize and recover input state [behavioral]
Plan: query all mapped axes and keys; publish initial snapshot before child launch; on SYN_DROPPED discard through SYN_REPORT and publish fresh snapshot. Fail invalid calibration queries. Tests use injected snapshots and event sinks without hardware.
- [x] 🔴 impl: startup, overflow, query failure tests
- [x] 🔴 ver: missing Snapshot/Relay APIs
- [x] 🟢 impl: snapshot queries and recovery relay
- [x] 🟢 ver: 49 tests; fmt; clippy -D warnings
- [x] 📦 commit: review clean; hash 5a88c23

## 3. Always enable rumble; remove disabling flag [behavioral]
Plan: delete option and related tests/docs/conditional setup. Require physical FF_RUMBLE with clear error; always advertise virtual rumble. Read/write access remains necessary. Test unconditional virtual capabilities and remaining CLI options.
- [x] 🔴 impl: unconditional setup and capability error tests; remaining CLI expectations
- [x] 🔴 ver: old boolean setup/options incompatible with new contract
- [x] 🟢 impl: flag removed; required physical rumble; always-on virtual capability; docs
- [x] 🟢 ver: 50 tests; fmt; clippy -D warnings; release build; no flag remnants
- [x] 📦 commit: review clean; hash c615ba1

Final validation: 50 tests, cargo fmt --check, clippy all-targets -D warnings, release build pass (host toolchain via flatpak-spawn). Updated: 2026-10-27.
No PR created. Physical input, FF ioctl success and actual device removal need hardware validation.
