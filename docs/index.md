---
layout: home

hero:
  name: "Fabrik"
  text: "Optimize software operations."
  tagline: Fabrik gives developers and agents one execution layer that maps software operations onto cache and compute infrastructure.
  image:
    src: /logo.png
    alt: Fabrik
  actions:
    - theme: brand
      text: What is Fabrik
      link: /guide/what-is-fabrik
    - theme: alt
      text: Scripts
      link: /guide/cacheable-scripts
    - theme: alt
      text: View on GitHub
      link: https://github.com/tuist/fabrik
features:
  - title: One surface for developers and agents
    details: Describe software operations once. Fabrik keeps the graph inspectable, editable, and debuggable instead of scattering work across wrappers and tool-specific glue.
    link: /guide/what-is-fabrik
    linkText: Read the definition
  - title: A real cache contract
    details: Actions carry explicit inputs, outputs, environment dependencies, and runtime semantics, so reuse is deterministic instead of accidental.
    link: /guide/cache
    linkText: Learn about caching
  - title: A portable compute contract
    details: The same operational definition can stay on the host today and map onto other execution backends as the infrastructure evolves.
  - title: Scripts without rewrites
    details: Checked-in scripts do not need to become bespoke rules. Fabrik can learn just enough about them to cache, inspect, and schedule them safely.
    link: /guide/cacheable-scripts
    linkText: Learn about scripts
---
