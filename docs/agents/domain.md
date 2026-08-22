# Domain docs

This repository uses a single-context domain-doc layout.

## Before exploring

Read these files when they exist:

- `CONTEXT.md` at the repository root
- Relevant ADRs under `docs/adr/`

If they do not exist, proceed without flagging their absence. The domain-modeling skill creates them when the project resolves terms or architectural decisions.

## File structure

The expected layout is:

/
├── CONTEXT.md
├── docs/adr/
└── crates/

## Vocabulary

Use domain terms as defined in `CONTEXT.md`. Do not replace defined terms with synonyms.

If a needed concept is missing, reconsider whether the project uses another term or note the gap for the domain-modeling skill.

## ADR conflicts

If proposed work contradicts an existing ADR, identify the conflict instead of silently overriding the decision.
