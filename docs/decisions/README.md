# Architecture Decision Records

This directory holds Fabrik's ADRs. They exist to make load-bearing
decisions reviewable in isolation, with their rationale visible long
after the people who made them have moved on.

## When to write one

Write an ADR when a decision:

1. Is hard to undo (changing it later requires migration, cache
   invalidation, or breaking changes), and
2. Several reasonable people would pick differently, and
3. The reasons for the choice are not obvious from the resulting code.

Plenty of decisions don't need an ADR. Code style, library choice
between near-equivalents, the shape of a private function. Use one
when a future maintainer reading just the code would reasonably ask
"why did they do it this way?" and the answer is non-trivial.

## Format

One markdown file per decision, numbered four digits. The file name is
`NNNN-short-slug.md`. Numbers never get reused even when an ADR is
superseded.

Required sections:

- **Status**: `Proposed`, `Accepted`, `Superseded by ADR-NNNN`, or
  `Deprecated`.
- **Context**: what problem we're solving, what constraints apply,
  what we already know.
- **Decision**: what we're going to do, in the imperative.
- **Consequences**: both what gets better and what gets worse.
- **Alternatives considered**: the options we rejected and why.

Optional but encouraged:

- **References**: links to specs, prior art, the research that
  informed the decision.
- **Open questions**: things this ADR explicitly does not settle.

## Lifecycle

ADRs get reviewed via PR like any other change. Once an ADR is
`Accepted`, it is immutable except for status changes. When a later
decision overrides it, write a new ADR and mark the old one
`Superseded by ADR-NNNN`. Don't edit the old one's body.

## Index

| # | Title | Status |
|---|---|---|
| [0001](0001-action-shape.md) | Action shape with typed inputs, typed outputs, and integration mode as a first-class field | Proposed |
