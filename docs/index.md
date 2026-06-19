---
layout: home

hero:
  name: "Once"
  text: "Run once. Reuse anywhere."
  tagline: Once turns repository automation into graph-aware, content-addressed actions with explicit inputs, outputs, environment, and runtime metadata.
  image:
    src: /logo.png
    alt: Once
  actions:
    - theme: brand
      text: Why
      link: /guide/why
    - theme: alt
      text: SDK
      link: /guide/sdk/rust
    - theme: alt
      text: Graph
      link: /guide/graph/
features:
  - title: Graph-aware automation
    details: Model work as targets with build, run, and test capabilities that agents and humans can query before executing anything.
  - title: Flexible action adapters
    details: Keep existing scripts and tools in place while they lower into the same content-addressed action model as typed target kinds.
  - title: Deterministic cache keys
    details: Inputs, outputs, environment variables, working directories, and timeouts become part of the action contract.
  - title: Providers
    details: Provider implementation is decoupled from actions, so teams can choose local cache storage, Tuist, and future remote compute providers.
---
