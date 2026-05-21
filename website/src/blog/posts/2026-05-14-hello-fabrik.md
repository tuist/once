---
title: "Hello, Fabrik"
description: "Why we are building a new build system in 2026, what works today, and what does not yet."
date: 2026-05-14
author: "The Fabrik team"
---

Welcome to the Fabrik blog. We will use it for release notes, design decisions, and the occasional field note from the workshop.

This first post is a short answer to the question we keep getting asked. Why a new build system in 2026?

## The shape of the problem

The tools most teams reach for to build, test, and run code today were designed for one of two worlds. A single language with strong conventions, where the package manager is the build system and nothing else needs to exist. Or a giant monorepo at a company with a build platform team and a years-long migration onto something like Bazel.

Most teams are neither. They ship in more than one language. The per-language tool stops scaling the moment a second language shows up. The monorepo tool is too much commitment for the size of the problem. The middle is mostly bespoke scripts that nobody wants to own. That middle is where Fabrik aims.

A second shift is happening at the same time. Coding agents are part of how teams ship now, and an agent watching a build needs more than a wall of stdout to decide what happened. It needs structured signals. Which target failed. What inputs it ran against. Which cache it hit. **A build system that is opaque to agents is a build system whose tooling stops at the human.** We do not think that holds for long.

## What it looks like today

```sh
fabrik init
fabrik build //app:hello
```

The granular path covers `rust.binary`, `rust.library`, `rust.test`, `rust.proc_macro`, and Elixir on OTP. Where the granular path does not yet cover what your existing toolchain handles, `cargo.binary` runs it in the cooperative model as a single cached action. You can start using Fabrik without rewriting your build to do it.

Fabrik builds Fabrik. The same graph that ships the binary verifies every change.

## The edges, named

It is the first cut, and there are things we would rather name than hide.

Build script support and per-target feature resolution for third-party Rust dependencies are the open items needed to self-host Fabrik on the granular pipeline end to end. The Apple and Elixir frontends are first-pass: they build, but the cooperative path will carry production work for a while. Plugins are on the roadmap, not in the box.

None of these are conceptual dead ends. They are the next few weeks.

## Where this is going

We are going to keep filling in the picture. More languages on the granular path. More inspectable structure for agents. Fewer reasons for the cooperative path to exist. Subscribe to the [RSS feed](/feed.xml) to follow along.
