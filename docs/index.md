---
layout: home

hero:
  name: "Fabrik"
  text: "A polyglot automation kernel for humans and agents."
  tagline: Builds, tests, scripts, generators. Fabrik decides what runs, where, and whether it is already done. Content-addressed cache, remote execution, deterministic outputs.
  image:
    src: /logo.png
    alt: Fabrik
  actions:
    - theme: brand
      text: What is Fabrik
      link: /guide/what-is-fabrik
    - theme: alt
      text: View on GitHub
      link: https://github.com/tuist/fabrik

features:
  - title: Polyglot targets
    details: Different languages and platforms share one declaration model. Extend the graph through plugins without fragmenting the build.
    link: /targets/rust
    linkText: Browse targets
  - title: Content-addressed cache
    details: Every action has an explicit input digest, declared outputs, and a deterministic environment. Local hits and remote reuse work from the same contract.
  - title: Built for coding agents
    details: Structured diagnostics, causality APIs, debug capsules, and repair flows make builds inspectable and editable by agents and humans alike.
  - title: Governed together
    details: Fabrik pairs open governance with first-party rules authored and maintained by us, so quality is not pushed onto the community alone.
---
