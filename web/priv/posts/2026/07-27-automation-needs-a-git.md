%{
  title: "Automation needs a git",
  description: "Why I started building Once, and the substrate it has always felt to me that automation was missing.",
  authors: ["pedro"]
}
---

Since we started building [Tuist](https://tuist.dev) as a business, there's one thing I've kept coming back to, almost obsessively: automation efficiency. We began with Xcode, because that's where we had presence and where we understood the pain deeply, but in the back of my head I kept turning over a more generic and more accessible version of the idea, one where Tuist could be the infrastructure provider for organizations of any shape, not just the ones building for Apple platforms. Two things turned that background thought into something I couldn't leave alone. Coding agents started increasing the concurrency and the sheer volume of code teams produce, and around the same time I watched Shopify build a solution on Nix. That was the wake-up call. It made me want to explore what a different world might actually look like.

I'm a builder at heart. I like throwing myself at problems that are supposedly already solved, as long as I feel I might do them better. Sometimes the honest conclusion is that I can't, but I only get there by getting my hands dirty, and I can only trust it once I've felt it myself. It's the same reason I enjoy watching what [DHH](https://x.com/dhh) and Jeff do with [Rails](https://rubyonrails.org), [Mise](https://mise.jdx.dev), and the rest of their projects. So it's not that we lack solutions here. Bazel and Buck exist, and they're remarkable pieces of engineering. But there's an aura around them, and a pattern I kept noticing where companies quietly walk away from them the moment the people who championed them move on. That pattern scratched at me. It made me want to understand whether a more approachable version of the same idea was possible.

That's how I started laying the first stones of Once. The timing helped, because we had just landed Bazel APIs on the server side, something I could plug straight into. And right around then I came across [this post](https://x.com/steipete/status/2050140050168451286?s=20) by Peter Steinberger, where he'd started reaching for remote compute to get around the limits of local compute. That's exactly what you'd expect the moment you have many parallel processes fighting over the same cores. Everything clicked.

## The GitHub of build systems

Early on, someone from a company in the Bazel space shared their ambition with us. **They wanted to be the GitHub of build systems.** It was the pitch they had used to raise funding.

I remember sitting with that sentence and feeling that something didn't quite line up. [GitHub](https://github.com) owes a good part of its success to Git, an open and standard versioning technology that didn't just make GitHub possible, it made [GitLab](https://gitlab.com) possible too, and a whole ecosystem around it. A GitHub of automation would need a substrate of the same kind. Something the community evolves rather than something a single company steers, a technology designed to plug into the ecosystems and infrastructure vendors people already use, a commons that could lift all of us to a better place instead of pulling everyone into one company's orbit. That framing became the thing I kept measuring ideas against. Not a product designed to lock people in, but a substrate worth building on top of.

## An incremental path to Starlark

My first explorations gravitated toward [Starlark](https://github.com/bazelbuild/starlark). There's something dynamic about it that makes it a genuinely good fit for this kind of job, and it has history on its side: it was designed for Bazel, and Buck uses it too. But Starlark is also a language most developers don't feel comfortable writing or maintaining, and one that agents often confuse with Python. So I kept asking myself a slightly different question. What if Starlark were opt-in, and underneath it we had something more static that mapped cleanly onto the internals Once already uses?

That question led me somewhere I didn't expect: maybe scripts can be cacheable too. The idea grew out of Steinberger's need, and also out of some API design I'd admired in [usage](https://usage.jdx.dev/). Maybe you don't need to throw yourself fully into replacing your build system. Maybe you can decorate the scripts you already have with a few annotations that make them remotely executable and cacheable. That's how the shell-based API came to life. Starlark is still fully there for teams that want its expressiveness, and I see the shell-based API not as a replacement for it but as a more incremental path toward it, a gentle way in for organizations that want remote caching and execution without tearing out their native toolchains.

In practice it stays close to a script you'd already have. You keep the script as the source of truth and add a small header that tells Once which inputs affect the result, which outputs to preserve, and which host facts belong in the cache key. Here's a script that installs a project's npm dependencies:

```sh
#!/usr/bin/env -S once exec -- bash
# once input "../package.json"
# once input "../package-lock.json"
# once output "../node_modules/"
# once fingerprint "node --version"
# once cwd ".."

npm ci
```

Those `# once` lines are the whole cache contract. The manifest, the lockfile, and the Node.js version form the key, and `node_modules/` is what gets restored on a hit. The first run installs and reports a miss, and every run after that restores the same `node_modules/` until one of those inputs changes. The same shape wraps a Vitest run, or a build, or whatever you already have, and once a script declares its outputs another one can depend on it through a `needs` header, which is how a remote test picks up that `node_modules/` without dragging the rest of your workspace along with it. If you want the details, it's all in the [scripted workflows guide](/docs/guide/scripted).

And that hesitation is fair. Replacing a toolchain with an alternative carries a real cost, one that shouldn't go unnoticed. It's exciting from an engineering standpoint, but companies deserve to name that cost, sit with it, and take their time before making the call. I'd rather meet them where they are than ask them to leap.

## Supporting ecosystems natively

From there I moved on to supporting ecosystems natively, and I started with [Rust](https://www.rust-lang.org) so I could replace the build of Once itself, which is written in Rust, with Once. From Buck 2 I borrowed the idea of shipping ecosystem support in a prelude that we own and maintain. We believe the most popular ecosystems should come with first-class support from Once, so that's the bar we set for ourselves.

The second half of that work mattered just as much. Starlark powers the graph, but we exposed a tool-based API on top of it so agents can declare and validate changes through the CLI and the MCP. We put a lot of care into the MCP server, and we followed every piece of work with a headless session where an agent tried to go from zero to a Once-defined Rust setup, or from an existing Rust project to a Once one. That loop is what surfaced the gaps. The goal was simple to say and harder to earn: anyone, at any moment, should be able to drop Once into a project, tell their coding agent "I want to speed this up with Once," and watch the agent actually finish the job. This is precisely the thing people kept complaining about with Bazel, so getting it right felt non-negotiable. That work became the foundation for everything that followed, and we wired it into Tuist's own infrastructure to speed up Rust compilation with remote cache. Remote execution will follow.

What the agent ends up writing is a `once.toml` next to the code. A crate, a binary that depends on it, and a test alongside them look like this:

```toml
[[target]]
name = "greeting"
kind = "rust_library"
srcs = ["src/lib.rs"]

[target.attrs]
crate_name = "greeting"
edition = "2021"

[[target]]
name = "hello"
kind = "rust_binary"
srcs = ["src/main.rs"]
deps = ["./greeting"]

[target.attrs]
crate_name = "hello"
edition = "2021"

[[target]]
name = "greeting_tests"
kind = "rust_test"
srcs = ["tests/greeting_test.rs"]
deps = ["./greeting"]

[target.attrs]
crate_name = "greeting_tests"
edition = "2021"
```

That's the whole declaration. The `./greeting` dependency is what tells the compiler which built crate to link, and from there `once build`, `once run`, and `once test` work against the graph while everything they produce stays cacheable and shareable. Third-party crates come in through a `cargo_dependencies` target that lets Cargo resolve from your `Cargo.toml` and `Cargo.lock` while Once builds the resolved crates as graph dependencies. The full walkthrough lives in the [Rust guide](/docs/guide/graph/rust).

From Rust we expanded outward, to ecosystems we know well and ones we wanted to learn: [Apple](https://developer.apple.com), which we're deeply familiar with, Elixir, which we use to build our web apps, [Go](https://go.dev), and [Zig](https://ziglang.org). The rules for Bazel and Buck, together with the native build systems, turned out to be a great reference for the agents, which did the actual implementation of the rules and validated the same two flows over and over, zero to setup and existing setup to Once, until we could build and test real projects in each language. We spent time tuning the cache interactions too, because the point isn't a new config file, it's a boost in productivity teams can feel.

This is the point where it started to feel like it could become a real automation substrate, with the kind of developer and agent experience I believe can earn broader adoption and trust.

## Where this is headed

There's still a lot of work ahead, but the direction feels right. We want to ship first-party rules for building [OCI images](https://opencontainers.org/). We want to invest in supporting ecosystems with no migration at all, so that if your `Cargo.toml` is already the source of truth for your graph, we adapt it into Once primitives and bring remote caching and execution without asking you to change a thing. I'd also like to explore what observability should look like here. Bazel has a protocol for it, and I want to find out whether it fits or whether we need something of our own. On the remote cache side, Bazel's protocol felt like the natural choice, so that's where we started.

It's early, and I'm sharing this while it's still taking shape. But GitHub had Git, and it has always felt to me like automation was missing a Git of its own. I think the timing is finally right to build it, because once every team is writing code quickly, the validation has to keep up with that pace.
