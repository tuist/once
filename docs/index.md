---
layout: home

hero:
  name: "Once"
  text: "Run once. Reuse anywhere."
  tagline: Once makes project scripts cacheable, observable, and remotely executable.
  image:
    src: /logo.png
    alt: Once
  actions:
    - theme: brand
      text: What is Once
      link: /guide/what-is-once
    - theme: alt
      text: Cacheable Scripts
      link: /guide/cacheable-scripts
    - theme: alt
      text: View on GitHub
      link: https://github.com/tuist/once
features:
  - title: Scripts stay scripts
    details: Keep the shell, Node, Python, Ruby, or other scripts you already have. Add a small execution contract instead of rewriting them as build rules.
    link: /guide/cacheable-scripts
    linkText: Learn about scripts
  - title: Deterministic cache keys
    details: Inputs, outputs, environment variables, working directories, and timeouts become part of the action contract.
  - title: Remote execution when it helps
    details: The same script contract can run locally or on a compute provider with `--remote`.
  - title: Runtime API for agents
    details: Runtime sessions expose structured logs, events, and control over JSON-RPC instead of forcing agents to scrape terminal output.
---
