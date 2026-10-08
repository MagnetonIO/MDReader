# Android debug artifact footprint

The completed arm64 development APK is **517,414,088 bytes** (SHA256
`1fc505c9fd8c1ea11d5f814cc12fec16b9d60b20dffbd32a0ad080a4ab6703f1`).
This is build-only evidence: it has not been installed/exercised, and its release
source-stamp provenance has not been verified. The installed baseline is untouched.

Inspecting the existing ZIP directory and ELF section table requires no rebuild,
model inference or source/profile access. The native library is stored without
ZIP compression and occupies 509,444,392 bytes. Its sections contain:

| Section category | Bytes |
| --- | ---: |
| `.debug*` information | 426,605,954 |
| `.symtab` and `.strtab` | 31,225,258 |
| Allocated, file-backed sections | 51,608,994 |

Debug information and ordinary symbol/string tables account for about 90% of
the native file. Android's debug build explicitly retains native symbols via
`jniLibs.keepDebugSymbols`; the release block does not set that retention rule.
This explains most of this debug artifact's size. It does not establish a release
APK size, an optimized stripped size or runtime resident memory.

Release acceptance must inspect an actual optimized, signed artifact and its
dependencies. Legacy CatDB code still appears in the app manifest and remains a
separate removal/migration gate. Repeated full packaging or changing compression
is not needed to establish these debug-section measurements.

The [machine-readable receipt](2026-10-07-android-debug-footprint.json) records
the observed package identity, largest entries/sections and evidence boundaries.
