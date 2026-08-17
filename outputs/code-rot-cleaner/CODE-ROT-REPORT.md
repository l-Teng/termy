# Code Rot Report

> **PROOF COMPLETE** — The real project was not changed.

Project: `/Users/lassevestergaard/Dev/termy`  
Generated: `2026-08-15T22:04:34Z`

## Executive summary

| Result | Candidates | LOC | Size |
|---|---:|---:|---:|
| SAFE TO REMOVE | 0 | 0 | 0 B |
| REVIEW | 5 | 3 | 120 B |
| KEEP | 19 | 3,115 | 96.8 KB |

## Proof status

Baseline in disposable copy: **PASSED**

| Command | Result | Duration |
|---|---|---:|
| `cargo check -p termy` | PASS | 5.800s |
| `cargo test -p termy --release` | PASS | 35.000s |
| `cargo fmt --all -- --check` | PASS | 0.200s |
| `just check-boundaries` | PASS | 10.900s |

## Ranked candidates

| ID | Status | Category | Subject | Confidence | Risk | LOC | Proof |
|---|---|---|---|---|---|---:|---|
| CRT-001 | **KEEP** | orphan-file | `.remember/tmp/last-ndc.ts` | high | low | 1 | .remember is untracked local tool state, not repository source; cleanup must not delete user tool data. |
| CRT-002 | **KEEP** | orphan-file | `crates/plugin_runtime/src/host.ts` | medium | medium | 661 | Embedded at compile time by include_str!("host.ts") in crates/plugin_runtime/src/lib.rs. |
| CRT-003 | **KEEP** | orphan-file | `crates/plugin_runtime/src/termy.d.ts` | medium | medium | 340 | Embedded at compile time by include_str!("termy.d.ts") in crates/plugin_runtime/src/lib.rs. |
| CRT-004 | **KEEP** | orphan-file | `crates/plugin_runtime/src/worker.ts` | medium | medium | 1631 | Embedded at compile time by include_str!("worker.ts") in crates/plugin_runtime/src/lib.rs. |
| CRT-005 | **KEEP** | orphan-file | `skills/build-termy-plugins/assets/command-plugin/plugin.ts` | medium | medium | 56 | Official command-plugin template referenced by skills/build-termy-plugins/SKILL.md and its examples reference. |
| CRT-006 | **KEEP** | orphan-file | `skills/build-termy-plugins/assets/native-ui-plugin/plugin.tsx` | medium | medium | 161 | Official native-UI plugin template referenced by skills/build-termy-plugins/SKILL.md and its examples reference. |
| CRT-007 | **KEEP** | orphan-file | `website/source.config.ts` | high | low | 22 | Fumadocs convention entrypoint consumed by postinstall, typecheck, and build tooling. |
| CRT-008 | **KEEP** | orphan-file | `website/src/components/markdown.tsx` | medium | medium | 167 | Imported by website/src/routes/releases/$slug.tsx. |
| CRT-009 | **KEEP** | orphan-file | `website/src/components/marketing-page-shell.tsx` | medium | medium | 165 | Imported by the home, download, and release route modules. |
| CRT-010 | **KEEP** | orphan-file | `website/src/components/mdx.tsx` | high | low | 15 | Imported by website/src/routes/docs/$.tsx and participates in MDX typing. |
| CRT-011 | **KEEP** | orphan-file | `website/src/components/not-found.tsx` | medium | medium | 11 | Imported by website/src/router.tsx as the default not-found component. |
| CRT-012 | **KEEP** | orphan-file | `website/src/lib/github-release.ts` | medium | medium | 289 | Imported by download and release route modules. |
| CRT-013 | **KEEP** | orphan-file | `website/src/lib/layout.shared.tsx` | medium | medium | 50 | Imported by docs routing and the not-found component. |
| CRT-014 | **KEEP** | orphan-file | `website/src/lib/sponsors.ts` | medium | medium | 36 | The sponsors value is imported by website/src/routes/index.tsx. |
| CRT-015 | **REVIEW** | unused-export | `marketingPanelClass from website/src/components/marketing-page-shell.tsx` | medium | medium | 0 | marketingPanelClass has no repository consumer and is a credible dead constant, pending website proof. |
| CRT-016 | **REVIEW** | unused-export | `GitHubReleaseYearGroup from website/src/lib/github-release.ts` | medium | medium | 0 | GitHubReleaseYearGroup is used only inside its file; removing export narrows the module surface but needs typecheck proof. |
| CRT-017 | **REVIEW** | unused-export | `appName from website/src/lib/shared.ts` | medium | medium | 0 | appName has no repository consumer and is a credible dead constant, pending website proof. |
| CRT-018 | **REVIEW** | unused-export | `Sponsor from website/src/lib/sponsors.ts` | medium | medium | 0 | Sponsor is used only inside its file; removing export narrows the module surface but needs typecheck proof. |
| CRT-019 | **KEEP** | unused-export | `FileRouteTypes from website/src/routeTree.gen.ts` | medium | medium | 0 | Generated TanStack Router declaration; repository instructions prohibit hand-editing routeTree.gen.ts. |
| CRT-020 | **KEEP** | unused-export | `FileRoutesByFullPath from website/src/routeTree.gen.ts` | medium | medium | 0 | Generated TanStack Router declaration; repository instructions prohibit hand-editing routeTree.gen.ts. |
| CRT-021 | **KEEP** | unused-export | `FileRoutesById from website/src/routeTree.gen.ts` | medium | medium | 0 | Generated TanStack Router declaration; repository instructions prohibit hand-editing routeTree.gen.ts. |
| CRT-022 | **KEEP** | unused-export | `FileRoutesByTo from website/src/routeTree.gen.ts` | medium | medium | 0 | Generated TanStack Router declaration; repository instructions prohibit hand-editing routeTree.gen.ts. |
| CRT-023 | **KEEP** | unused-export | `RootRouteChildren from website/src/routeTree.gen.ts` | medium | medium | 0 | Generated TanStack Router declaration; repository instructions prohibit hand-editing routeTree.gen.ts. |
| CRT-024 | **REVIEW** | unused-dependency | `p256 and rand_core direct dependencies from termy` | high | low | 3 | Static evidence and disposable-copy proof agree: the exact manifest and lockfile cleanup passed desktop check, 805 tests, formatting, boundaries, and cargo-machete. |

## Evidence by candidate

### CRT-001 — KEEP

- Location: `.remember/tmp/last-ndc.ts`
- Category: `orphan-file`
- Potential size: 1 LOC / 11 B
- Status reason: .remember is untracked local tool state, not repository source; cleanup must not delete user tool data.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability.

### CRT-002 — KEEP

- Location: `crates/plugin_runtime/src/host.ts`
- Category: `orphan-file`
- Potential size: 661 LOC / 19.6 KB
- Status reason: Embedded at compile time by include_str!("host.ts") in crates/plugin_runtime/src/lib.rs.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: host.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-003 — KEEP

- Location: `crates/plugin_runtime/src/termy.d.ts`
- Category: `orphan-file`
- Potential size: 340 LOC / 8.7 KB
- Status reason: Embedded at compile time by include_str!("termy.d.ts") in crates/plugin_runtime/src/lib.rs.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: crates/plugin_runtime/src/termy.d, crates/plugin_runtime/src/termy.d.ts, termy.d, termy.d.ts.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-004 — KEEP

- Location: `crates/plugin_runtime/src/worker.ts`
- Category: `orphan-file`
- Potential size: 1,631 LOC / 54.9 KB
- Status reason: Embedded at compile time by include_str!("worker.ts") in crates/plugin_runtime/src/lib.rs.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: worker.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-005 — KEEP

- Location: `skills/build-termy-plugins/assets/command-plugin/plugin.ts`
- Category: `orphan-file`
- Potential size: 56 LOC / 1.5 KB
- Status reason: Official command-plugin template referenced by skills/build-termy-plugins/SKILL.md and its examples reference.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: plugin, plugin.ts.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-006 — KEEP

- Location: `skills/build-termy-plugins/assets/native-ui-plugin/plugin.tsx`
- Category: `orphan-file`
- Potential size: 161 LOC / 4.9 KB
- Status reason: Official native-UI plugin template referenced by skills/build-termy-plugins/SKILL.md and its examples reference.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: plugin, plugin.tsx.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-007 — KEEP

- Location: `website/source.config.ts`
- Category: `orphan-file`
- Potential size: 22 LOC / 422 B
- Status reason: Fumadocs convention entrypoint consumed by postinstall, typecheck, and build tooling.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability.

### CRT-008 — KEEP

- Location: `website/src/components/markdown.tsx`
- Category: `orphan-file`
- Potential size: 167 LOC / 5.0 KB
- Status reason: Imported by website/src/routes/releases/$slug.tsx.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: markdown.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-009 — KEEP

- Location: `website/src/components/marketing-page-shell.tsx`
- Category: `orphan-file`
- Potential size: 165 LOC / 5.2 KB
- Status reason: Imported by the home, download, and release route modules.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: marketing-page-shell.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-010 — KEEP

- Location: `website/src/components/mdx.tsx`
- Category: `orphan-file`
- Potential size: 15 LOC / 386 B
- Status reason: Imported by website/src/routes/docs/$.tsx and participates in MDX typing.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability.

### CRT-011 — KEEP

- Location: `website/src/components/not-found.tsx`
- Category: `orphan-file`
- Potential size: 11 LOC / 304 B
- Status reason: Imported by website/src/router.tsx as the default not-found component.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: not-found.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-012 — KEEP

- Location: `website/src/lib/github-release.ts`
- Category: `orphan-file`
- Potential size: 289 LOC / 7.6 KB
- Status reason: Imported by download and release route modules.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: github-release.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-013 — KEEP

- Location: `website/src/lib/layout.shared.tsx`
- Category: `orphan-file`
- Potential size: 50 LOC / 1.1 KB
- Status reason: Imported by docs routing and the not-found component.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: layout.shared.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-014 — KEEP

- Location: `website/src/lib/sponsors.ts`
- Category: `orphan-file`
- Potential size: 36 LOC / 848 B
- Status reason: The sponsors value is imported by website/src/routes/index.tsx.
- Evidence: No resolved static inbound import was found. The file is not a detected package or conventional entry point. Repository text contains possible string/name references: sponsors.
- Caveats: Dynamic loading, reflection, external consumers, or incomplete framework detection can hide reachability. Possible string reachability prevents high-confidence deletion.

### CRT-015 — REVIEW

- Location: `website/src/components/marketing-page-shell.tsx:164`
- Category: `unused-export`
- Potential size: 0 LOC / 0 B
- Status reason: marketingPanelClass has no repository consumer and is a credible dead constant, pending website proof.
- Evidence: No reference to the exported symbol was found outside its defining file.
- Caveats: Public APIs, re-export patterns, generated declarations, templates, or external consumers may use it.

### CRT-016 — REVIEW

- Location: `website/src/lib/github-release.ts:208`
- Category: `unused-export`
- Potential size: 0 LOC / 0 B
- Status reason: GitHubReleaseYearGroup is used only inside its file; removing export narrows the module surface but needs typecheck proof.
- Evidence: No reference to the exported symbol was found outside its defining file.
- Caveats: Public APIs, re-export patterns, generated declarations, templates, or external consumers may use it.

### CRT-017 — REVIEW

- Location: `website/src/lib/shared.ts:1`
- Category: `unused-export`
- Potential size: 0 LOC / 0 B
- Status reason: appName has no repository consumer and is a credible dead constant, pending website proof.
- Evidence: No reference to the exported symbol was found outside its defining file.
- Caveats: Public APIs, re-export patterns, generated declarations, templates, or external consumers may use it.

### CRT-018 — REVIEW

- Location: `website/src/lib/sponsors.ts:4`
- Category: `unused-export`
- Potential size: 0 LOC / 0 B
- Status reason: Sponsor is used only inside its file; removing export narrows the module surface but needs typecheck proof.
- Evidence: No reference to the exported symbol was found outside its defining file.
- Caveats: Public APIs, re-export patterns, generated declarations, templates, or external consumers may use it.

### CRT-019 — KEEP

- Location: `website/src/routeTree.gen.ts:102`
- Category: `unused-export`
- Potential size: 0 LOC / 0 B
- Status reason: Generated TanStack Router declaration; repository instructions prohibit hand-editing routeTree.gen.ts.
- Evidence: No reference to the exported symbol was found outside its defining file.
- Caveats: Public APIs, re-export patterns, generated declarations, templates, or external consumers may use it.

### CRT-020 — KEEP

- Location: `website/src/routeTree.gen.ts:68`
- Category: `unused-export`
- Potential size: 0 LOC / 0 B
- Status reason: Generated TanStack Router declaration; repository instructions prohibit hand-editing routeTree.gen.ts.
- Evidence: No reference to the exported symbol was found outside its defining file.
- Caveats: Public APIs, re-export patterns, generated declarations, templates, or external consumers may use it.

### CRT-021 — KEEP

- Location: `website/src/routeTree.gen.ts:90`
- Category: `unused-export`
- Potential size: 0 LOC / 0 B
- Status reason: Generated TanStack Router declaration; repository instructions prohibit hand-editing routeTree.gen.ts.
- Evidence: No reference to the exported symbol was found outside its defining file.
- Caveats: Public APIs, re-export patterns, generated declarations, templates, or external consumers may use it.

### CRT-022 — KEEP

- Location: `website/src/routeTree.gen.ts:79`
- Category: `unused-export`
- Potential size: 0 LOC / 0 B
- Status reason: Generated TanStack Router declaration; repository instructions prohibit hand-editing routeTree.gen.ts.
- Evidence: No reference to the exported symbol was found outside its defining file.
- Caveats: Public APIs, re-export patterns, generated declarations, templates, or external consumers may use it.

### CRT-023 — KEEP

- Location: `website/src/routeTree.gen.ts:138`
- Category: `unused-export`
- Potential size: 0 LOC / 0 B
- Status reason: Generated TanStack Router declaration; repository instructions prohibit hand-editing routeTree.gen.ts.
- Evidence: No reference to the exported symbol was found outside its defining file.
- Caveats: Public APIs, re-export patterns, generated declarations, templates, or external consumers may use it.

### CRT-024 — REVIEW

- Location: `crates/desktop_app/Cargo.toml`
- Category: `unused-dependency`
- Potential size: 3 LOC / 120 B
- Status reason: Static evidence and disposable-copy proof agree: the exact manifest and lockfile cleanup passed desktop check, 805 tests, formatting, boundaries, and cargo-machete.
- Evidence: cargo-machete independently reports both direct dependencies unused by the termy crate. Repository-wide source search finds no p256, rand_core, OsRng, RngCore, signing-key, or verifying-key use in crates/desktop_app. Git history shows both dependencies entered with the former embedded-browser surface, which was removed by fbe5f62c.
- Caveats: The workspace-level rand_core dependency must remain because termy_plugin_runtime uses it. Cargo.lock changes and compilation must be proven in a disposable copy before applying the manifest cleanup.

## Cleanup approval checklist

No cleanup has been applied. To continue, select exact candidate IDs and review their paths, evidence, proof, and residual risk. Manifest or lockfile changes require separate explicit approval.

```text
Approved candidate IDs: ____________________
Approved files / manifest entries: __________
Approved verification commands: _____________
```

## Scope and limitations

- Scanned 32 source files, 6,174 LOC, 191.6 KB.
- Static analysis cannot prove absence of dynamic, reflective, operational, platform-specific, or external use.
- JavaScript/TypeScript and Python import resolution is intentionally conservative and does not implement every alias or framework convention.
- Unused dependencies and exports are leads only until project-native tooling and focused proof support removal.
- No project code, package command, or dependency was executed by this scanner.
- Commands ran in disposable copies, not the real project.
- The first sandboxed GPUI build attempts could not write the Metal module cache; the same commands passed when rerun with approved cache access.
- A green command set proves only the behavior exercised by those commands.
