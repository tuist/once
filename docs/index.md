---
layout: home

hero:
  name: "Fabrik"
  text: "A build system for humans and agents."
  tagline: Polyglot, agent-native, content-addressed. One graph that humans and coding agents can build, run, test, and debug.
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
    link: /guide/cache-and-execution
    linkText: How caching works
  - title: Built for coding agents
    details: Structured diagnostics, causality APIs, debug capsules, and repair flows make builds inspectable and editable by agents and humans alike.
    link: /guide/agent-native
    linkText: Agent-native features
  - title: Governed together
    details: Fabrik pairs open governance with first-party rules authored and maintained by us, so quality is not pushed onto the community alone.
    link: /reference/rules
    linkText: Rules reference
---
