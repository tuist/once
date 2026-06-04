---
layout: home

hero:
  name: "Once"
  text: "Run once. Reuse anywhere."
  tagline: Once turns ordinary project scripts into content-addressed actions with explicit inputs, outputs, environment, and runtime metadata.
  image:
    src: /logo.png
    alt: Once
  actions:
    - theme: brand
      text: What is Once
      link: /guide/what-is-once
    - theme: alt
      text: Caching
      link: /guide/cacheable-scripts
    - theme: alt
      text: View on GitHub
      link: https://github.com/tuist/once
features:
  - title: Scripts stay scripts
    details: Keep the shell, Node.js, Python, Ruby, or other scripts you already have. Add a small execution contract in the script itself.
    link: /guide/cacheable-scripts
    linkText: Learn about caching
  - title: Deterministic cache keys
    details: Inputs, outputs, environment variables, working directories, and timeouts become part of the action contract.
  - title: Shared cache providers
    details: Cache provider implementation is decoupled from scripts, so teams can choose local storage, Tuist, or another shared provider.
---
