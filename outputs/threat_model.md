# Overview

Termy is a cross-platform terminal product whose repository contains several distinct runtime surfaces: the Rust/GPUI desktop application, the reusable headless terminal runtime, a C-compatible FFI used by native hosts, an alternative Swift/macOS host, a local Bun plugin runtime, SSH and tmux integrations, update and release machinery, a hosted account API, and a public documentation/download website.

The highest-value assets are the user's local shell authority, terminal input and output, filesystem and process access inherited by the application, saved SSH connection metadata and keychain-backed credentials, trusted host-key decisions, local configuration and workspace state, installed application binaries, release credentials and artifacts, cloud-account sessions and account data, and the memory-safety and availability of native embedding hosts.

Primary product/runtime ownership is grounded in `crates/desktop_app`, `crates/core`, `crates/terminal_ui`, `crates/plugin_runtime`, `crates/ssh_core`, `crates/ffi`, `macos`, `crates/auto_update`, `crates/release_core`, `crates/api`, and `website`. Test, documentation, examples, and one-off developer tooling are secondary unless they feed release artifacts, generated public content, or privileged CI workflows.

# Threat Model, Trust Boundaries, and Assumptions

## Local user, terminal, and subprocess boundary

Termy runs with the desktop user's authority and launches local shells, PTYs, tmux, OpenSSH, installers, and plugin subprocesses. Terminal output must be treated as attacker-controlled whenever it can originate from an untrusted remote host, container, repository, command, or file. Escape sequences, OSC payloads, hyperlinks, Kitty graphics, title changes, tmux control messages, and very large scrollback/search results must not cross from terminal data into arbitrary local command execution, unsafe URL opening, unbounded allocation, or UI compromise.

User keystrokes, mouse events, clipboard actions, file drops, command-palette actions, configuration, task/layout definitions, and deep links are operator-controlled inputs. They are trusted to express user intent but not necessarily well-formed. The command boundary must preserve structured program-plus-argv execution where values can contain shell metacharacters. Configuration parsing and persistence must avoid turning malformed or attacker-planted files into execution or path-traversal primitives.

The security model assumes the operating system, the current desktop account, and the configured shell are trusted. An attacker who already has arbitrary code execution as the same user generally has equivalent or greater authority than Termy; issues that require that position are usually defense-in-depth unless Termy crosses into additional credentials, signing identities, elevated helpers, or other users' data.

## SSH and remote-host boundary

Remote SSH servers, DNS results, banners, host-key prompts, terminal output, and remote tmux state are untrusted. Saved host fields and identity-file paths are operator-controlled and may also come from imported local state. `termy_ssh_core` is expected to persist only non-secret metadata, keep passwords and passphrases in the system credential store, launch the system OpenSSH client with typed argv rather than a shell command, and leave `known_hosts` verification to OpenSSH.

The `SSH_ASKPASS` helper crosses a particularly sensitive process-authentication boundary. Non-secret routing metadata in its environment must not be enough for an unrelated local process to retrieve credentials. Parent-process validation, expected Termy process identity, prompt-kind restriction, stable host IDs, and keychain scoping are security controls. A local same-user attacker may still have broad inspection or keychain abilities, but confused-deputy access from unrelated processes remains in scope.

## Plugin boundary

Plugins are explicitly trusted local Bun code, not a security sandbox. Once loaded they may use filesystem, subprocess, network, Bun, and Node-compatible APIs with the user's authority. Manifest capabilities gate only Termy-owned APIs and native UI; they do not contain operating-system access. Therefore arbitrary OS access by an installed malicious plugin is expected behavior rather than a reportable sandbox escape.

Security-relevant invariants still exist around installation and host integration: GitHub or local installation must not execute code or build hooks before explicit load; source traversal, symlinks, and out-of-root imports must be rejected; package installation must not occur automatically; descriptor, action, document, and native-view data must be validated and bounded; unknown native nodes or props must not reach GPUI; protocol sizes and timeouts must limit resource exhaustion; and one failing Worker should not corrupt the host protocol or crash the desktop app. Plugin subprocesses can outlive Worker cancellation by design and should be treated as part of the trusted-plugin model.

## Native FFI and Swift host boundary

`termy_ffi` accepts raw pointers, lengths, UTF-8 strings, C-compatible structs, lifecycle calls, and terminal bytes from embedding hosts. It must keep the Rust header and ABI aligned, reject null or invalid inputs where promised, prevent Rust panics from unwinding across the ABI, return ownership through matching allocation/free functions, and document the handle's caller-serialized threading contract.

The embedding host is assumed to honor the documented ABI, lifetime, and serialization contract. Arbitrary misuse by a malicious in-process host already capable of memory corruption is generally out of scope. Reportable issues include safe-looking documented calls that cause memory unsafety, double free, use-after-free, cross-thread races despite documented compliance, panic escape, or unexpected access beyond the caller-provided buffers.

## Update, download, packaging, and CI boundary

Release metadata and artifacts arrive through GitHub and the public network. The updater must select the intended repository, platform, architecture, and asset; use authenticated transport; constrain download destinations; verify artifacts according to platform policy before execution or replacement; and avoid path, mount, archive, or command injection. Checksums fetched from the same compromised release account provide corruption detection but not an independent signature of publisher identity.

Packaging scripts and `.github/workflows` cross from developer-controlled source and CI inputs into distributable binaries. Repository contributors, action dependencies, release tags, workflow parameters, environment variables, and fetched toolchains are developer-controlled or supply-chain inputs. Release tokens, signing identities, notarization credentials, and uploaded artifacts are high-value. Untrusted pull requests must not gain secret-bearing release authority. The current unsigned macOS distribution weakens provenance and user verification and should influence severity when an issue enables release substitution.

## Hosted API and database boundary

`termy_api` is a network-facing Axum service. Unauthenticated internet clients control HTTP methods, paths, headers, cookies, auth payloads, and request timing. Authenticated clients control their own account fields and sessions but must not cross account boundaries. The database and environment are operator-controlled; `DATABASE_URL`, `TERMY_API_SECRET`, base URL, and port are deployment inputs.

Authentication, password management, session management, and account management are delegated to `better-auth`; Termy must configure origins/base URLs, cookie behavior, secrets, and proxy assumptions safely and must not bypass its session extractor on application routes. SQL must stay parameterized. Health endpoints should not leak credentials or sensitive topology. Production database connections must preserve confidentiality and authenticate the intended server; plaintext direct Postgres is acceptable only for the documented local/dev use case.

The API currently exposes a narrow application surface, so multi-tenant authorization risk is limited today, but future cloud-agent endpoints would make object-level authorization, tenant isolation, rate limiting, resource quotas, and auditability primary concerns.

## Website and browser boundary

The public website consumes repository MDX/content and GitHub release metadata and serves download links and documentation. Remote GitHub data, URL parameters, markdown/MDX rendering inputs, and browser requests are untrusted at runtime unless baked from reviewed repository content. The website must avoid unsafe HTML/URL rendering, open redirects, server-side request forgery, path traversal in content routes, and download substitution. Browser tabs inside the desktop app add a separate web-content-to-native boundary; web content must not gain arbitrary desktop command or local-file authority through navigation, custom schemes, link handling, or IPC.

## Persistence and availability boundary

Configuration, themes, SSH metadata, plugin state, workspace layouts, terminal buffers, crash logs, caches, and update downloads live in user-writable locations. A same-user attacker can often modify these files, but Termy must still parse them safely, constrain resolved paths to intended roots where promised, use atomic persistence for security-sensitive metadata, avoid following dangerous symlinks during managed install/uninstall/update flows, and bound file size, recursion, allocation, and expensive rendering/search work.

# Attack Surface, Mitigations, and Attacker Stories

- A remote process prints crafted terminal control sequences. Realistic impacts include unwanted URL activation, clipboard manipulation outside stated policy, parser state confusion, memory/CPU exhaustion, or injection into tmux control handling. Existing separation between reusable protocol/runtime code and UI adapters, explicit link handling, parser tests, and bounded rendering structures are relevant mitigations.
- A user connects to a malicious or compromised SSH host. The host can control terminal output and prompts but should not retrieve unrelated keychain credentials, disable host-key verification, or inject local shell syntax through saved host fields. Structured OpenSSH argv, keychain separation, prompt allowlisting, and parent validation mitigate this boundary.
- A user installs a malicious plugin. Full same-user OS access after load is expected. In-scope attacker stories instead include pre-load execution during download/discovery, escaping managed source roots, exploiting native-view validation, corrupting the persistent host protocol, or causing application-wide denial of service beyond documented trusted-code behavior.
- An attacker compromises release metadata, a release account, DNS/TLS assumptions, or a writable update cache. A successful story replaces a trusted Termy binary or abuses installer arguments/mount contents. Checksum verification, fixed repository selection, platform asset filtering, read-only mounts, typed process arguments, and CI packaging boundaries mitigate parts of this chain; code signing or an independent signature would provide stronger publisher authentication.
- An internet client targets `/auth` or future `/api` routes. Relevant classes are auth bypass, session fixation/theft, weak deployment secrets, cross-origin misconfiguration, brute-force/resource exhaustion, SQL injection, and cross-account object access. A 32-character secret floor, framework session extractors, typed SQL, and narrow current routes are mitigations, but deployment configuration remains part of the trust model.
- Malformed configuration, theme, workspace, plugin, tmux, deep-link, or FFI input triggers excessive allocation, blocking synchronous work, unsafe path resolution, panic, or memory corruption. Rust type and ownership safety reduces memory-corruption likelihood in pure Rust, but FFI, subprocess boundaries, large attacker-controlled buffers, recursive structures, and UI-thread work remain important.
- A malicious contributor modifies release automation or dependency resolution. The critical chain is untrusted change to secret-bearing CI, build scripts, action pinning, lockfiles, signing/notarization steps, or uploaded artifact selection. Review, branch protection, locked dependency resolution, pinned third-party actions/toolchains, artifact-path checks, and isolated release triggers are expected controls.

Lower-priority or generally out-of-scope stories include arbitrary behavior by an explicitly trusted loaded plugin, attacks requiring an already-compromised same-user desktop account with no additional privilege gained, malicious in-process FFI callers that violate the documented pointer/lifetime contract, and social engineering that asks the user to deliberately run arbitrary shell commands. These can become in scope when Termy claims containment, crosses into keychain/release/elevated authority, or performs the dangerous action without clear user intent.

# Severity Calibration (Critical, High, Medium, Low)

## Critical

- Remote, unauthenticated code execution on a user's machine from terminal output, website/browser content, update metadata, or an SSH server without a deliberate trusted-plugin or shell command action.
- Supply-chain compromise that lets an untrusted contributor or network attacker publish or install attacker-controlled binaries through official release/update channels.
- Internet-reachable API compromise exposing signing secrets, database-wide credentials, or all users' active sessions/accounts.

## High

- Cross-account authentication or authorization bypass in the hosted API with meaningful account data or action access.
- Silent retrieval of SSH passwords or private-key passphrases by an unrelated local process through a confused `SSH_ASKPASS` boundary.
- A memory-safety flaw reachable through documented FFI use, or remotely supplied terminal/tmux bytes, with plausible code-execution impact.
- Native browser or plugin UI data crossing into arbitrary desktop commands despite the documented boundary, excluding arbitrary actions by already trusted plugin code.
- Update artifact substitution or unsafe installer handling that requires a realistic but non-default local or network precondition.

## Medium

- Persistent unauthorized configuration, workspace, theme, plugin, or managed-file modification across a meaningful trust boundary.
- SSRF, SQL injection, origin/session weakness, or object-level authorization failure with constrained data or deployment reach.
- Terminal/tmux/OSC/deep-link input causing repeatable application crash, major data loss, or substantial resource exhaustion from a realistic remote source.
- Managed plugin installation or import validation escaping its intended root before code is explicitly trusted and loaded.
- Release or CI weakness that exposes secrets or artifact integrity only after a contributor-level or workflow-specific precondition.

## Low

- Bounded denial of service requiring large local/operator-controlled inputs, such as excessive search-result materialization or rendering work, when recovery is straightforward and no privilege boundary is crossed.
- Information exposure limited to non-secret local metadata, diagnostics, or process details.
- Robustness failures in malformed configuration, tmux state, plugin documents, or FFI errors that cause a contained crash or stale state without memory unsafety or durable data loss.
- Defense-in-depth gaps where exploitation requires arbitrary same-user code execution, a deliberately loaded trusted plugin, or violation of the documented FFI threading/lifetime contract and yields no additional authority.

Repository: target_sha256_fc134de514f5244f12d32c73cbf17f2b6df153560982c5b129beae4989653391
Version: 6b0d3e539f013d258e4be08b469f8381c0b68989
