%{
  title: "Scripts are a feature, not a fallback",
  authors: ~w(pedro),
  description: "Existing scripts provide a practical path into a typed and cacheable build graph."
}
---
Adopting a build system should not require rewriting every useful script on day one. Scripts already capture years of repository knowledge, and replacing them all at once creates risk without necessarily creating value.

Once treats scripts as a migration path into the build graph. A script can declare the files it reads, the outputs it produces, and the directory where it runs. Those declarations are enough to make the work cacheable and remotely executable.

The first step can stay direct:

```sh
once exec -- ./scripts/build
```

As the workflow settles, a team can move stable boundaries into typed targets. Dependencies become queryable, outputs become structured providers, and validation can catch mistakes before an action starts.

The result is an incremental path. Keep the scripts that work, make their contracts visible, and introduce deeper structure where it earns its place.
