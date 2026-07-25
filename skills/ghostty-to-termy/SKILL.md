---
name: ghostty-to-termy
description: Convert Ghostty terminal configuration files into valid Termy config.txt files, including supported appearance, colors, behavior, and keybindings, while reporting approximations and unsupported Ghostty settings. Use when migrating from Ghostty to Termy, translating a Ghostty config or theme, comparing Ghostty and Termy configuration support, or diagnosing why a migrated Termy setting is invalid.
---

# Convert Ghostty configuration to Termy

Produce a valid Termy config and a compatibility report. Never silently copy a
Ghostty key into Termy merely because the names look similar.

## Workflow

1. Locate the Ghostty input. Prefer an explicit path. Otherwise check:
   - `$XDG_CONFIG_HOME/ghostty/config.ghostty`
   - `$XDG_CONFIG_HOME/ghostty/config`
   - `~/.config/ghostty/config.ghostty`
   - `~/.config/ghostty/config`
   - macOS Application Support paths documented by Ghostty
2. Run the bundled converter:

   ```bash
   python3 scripts/convert_ghostty_to_termy.py SOURCE \
     --output /tmp/termy-config.txt \
     --report /tmp/termy-compatibility.json
   ```

3. Read the generated config and report. Resolve unsupported or approximated
   entries only when the user wants manual equivalents.
4. For any manual mapping, verify the target key and accepted value in
   [references/termy-llms-full.txt](references/termy-llms-full.txt). Search it
   with `rg -n` rather than loading the entire 76 KB file.
5. Validate structurally by rerunning the converter with `--strict`. If a Termy
   binary or config validation command is available, validate with it too.
6. Write to `~/.config/termy/config.txt` only when the user asks to install or
   replace their live config. Preserve an existing file unless replacement is
   explicit.

## Conversion rules

Follow [references/mapping-policy.md](references/mapping-policy.md) for exact
and lossy mappings.

- Keep ordinary Termy keys and repeated `keybind` lines before `[colors]`.
- Emit `[colors]` last because it is Termy's only section header.
- Normalize Ghostty `RRGGBB` colors to Termy's required `#RRGGBB`.
- Inline Ghostty `config-file` includes using Ghostty's load order.
- Preserve only behavior Termy supports. Put everything else in the report.
- Treat named Ghostty themes as unresolved unless their colors are made
  explicit. A matching theme slug does not prove the same theme exists in
  Termy.
- Treat Ghostty keybind prefixes, sequences, text/escape injection, global
  shortcuts, and parameterized actions as unsupported unless Termy documents
  an equivalent.
- Never invent Termy action names. Check the `# Actions` section in the bundled
  docs when adding a manual keybinding mapping.

## Output contract

Return:

- the generated Termy config path or config text;
- counts of exact, approximated, unsupported, and warning entries;
- a short list of lossy or unsupported settings that matter to the user;
- whether validation was structural only or also performed by Termy itself.

Do not call a migration complete when the report still contains important
unsupported behavior without telling the user.
