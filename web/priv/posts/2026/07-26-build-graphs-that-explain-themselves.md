%{
  title: "Build graphs that explain themselves",
  authors: ~w(pedro),
  description: "A useful build graph should be typed, queryable, and understandable before any work begins."
}
---
A build graph is more than an ordering of commands. It is an explanation of the repository: which targets exist, how they depend on each other, what each target can do, and which artifacts move between them.

Once keeps that explanation available through the command line and its tools for coding agents. A client can discover target kinds, fetch their contracts, validate a draft, and commit a structural edit without scraping prose documentation.

The graph stays ecosystem-neutral. Apple, Android, Rust, Go, and other toolchains express their behavior through target kinds that share the same typed model. The core provides the primitives for validation, actions, providers, and queries.

This structure lets a developer answer a small question without starting a build, and it gives an agent the context needed to make a small change without guessing.
