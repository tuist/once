%{
  title: "Why we are building Once",
  authors: ~w(pedro),
  description: "Repository automation should become faster, easier to inspect, and safer to reuse every time it runs."
}
---
Software teams keep writing the same kind of automation. A script builds an application, another checks its style, and a third prepares a release. The commands differ, but they all need to answer the same questions: what goes in, what comes out, and when can a previous result be reused?

Once makes those answers explicit. An action declares its inputs, outputs, tools, and environment. From that contract, Once can calculate a stable identity for the work, restore a matching result, or run it in a fresh environment when something changed.

That is useful for people, and it matters even more for coding agents. An agent should be able to inspect a repository's build graph, understand what a target can do, and make a focused edit without first reverse engineering a collection of hidden conventions.

We are building Once so repository automation gets more valuable with every run. A local build, a shared cache, and remote execution all use the same action model. Build once, reuse everywhere.
