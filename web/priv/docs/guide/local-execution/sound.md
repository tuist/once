# The Sound of Your Build

Watching a build for status messages is a poor use of your attention. `once`
can play the shape of a running command as a piece of music, so you can hear
whether things are moving, hitting the cache, or resolving without looking at
the terminal.

The feature is off by default. Add `--sound` to any command that runs actions
(`build`, `test`, `run`, `exec`, `lint`, and the compatibility wrappers for
`xcodebuild`, `swift`, `bazel`, and `cargo`) to turn it on for that
invocation.

```sh
once --sound build services/api/Api
once --sound test --all
once --sound exec -- mise install --yes
```

When the default audio device is unavailable (headless CI, no audio hardware)
the flag has no effect and the command runs as if it had not been passed.

On Linux, audio output goes through ALSA, and `once` links against it when it
loads, whether or not you pass `--sound`. Minimal images such as
`debian:*-slim` do not ship it, so install `libasound2` there before running
`once`:

```sh
apt-get update && apt-get install -y libasound2
```

## What You Hear

Each run is one continuous piece of music. It has a beginning, a middle, and
an end, and it evolves in response to what the run is doing.

### A musical fingerprint per command

Before a run starts, `once` derives a fingerprint from the command being run
(the verb, the target id, or the argv for a low-level exec) and uses it to
seed the piece. Same command means same fingerprint, which means the same
musical identity every time. Different commands get their own piece.

The fingerprint chooses:

- **Root of the chord progression.** One of I, ii, iii, IV, V of C major, so
  every build stays tonal.
- **Chord progression variant.** One of four slow chord walks.
- **Arpeggio base tempo.** Between 2.4 and 4.0 seconds per note.
- **Shimmer register.** The airy top layer sits at C5 or G5.

`once build services/api/Api` will always sound like `services/api/Api`.
`once test --all` and `once build services/api/Api` will sound different.

### A pad that evolves through the run

The bed underneath the piece is a continuous evolving pad. It has three
layers stacked on top of each other:

- A deep drone at the pad's root plus a fifth, with a slow tape-like pitch
  wobble.
- A mid triad that walks through the chosen chord progression, one chord
  every eleven seconds or so, gliding between positions rather than jumping.
- An airy shimmer with a slow tremolo.

Five free-running LFOs at incommensurate rates (0.028, 0.055, 0.08, 0.19, and
0.4 Hz) shape those layers in parallel. Because none of the rates divide any
of the others cleanly, the interference pattern never lands twice in the same
place, and the pad drifts continuously instead of looping.

A quiet arpeggio picks through the current chord tones an octave above the
mid pad. Its tempo is not fixed: it quickens when the build has recent
activity and slows back down when things go quiet.

### Events reshape the pad

The pad is not a static backdrop that events sit on top of. Each event does
two things at once. It plays a distinct musical gesture, and it persistently
reshapes the pad through four dimensions that decay slowly over the next
fifteen to twenty seconds:

- `brightness`, raised by cache hits. The shimmer layer gets brighter and
  the pad reads as glassier and more present.
- `warmth`, raised by fresh action executions. The mid pad thickens.
- `depth`, raised by failures. The drone deepens and the pad settles darker.
- `density`, raised by any activity. The arpeggio quickens.

The result is that a cache-heavy build glows and rings. A build that does
lots of fresh work grows warm and thick. A build that fails sinks low.

### The events themselves

- **Fresh action ran.** A short brass stab in the mid range.
- **Cache hit.** A small ringing bell chord (C, E, G in the shimmer octave)
  plus a high sparkle. Cache hits are the whole point of a build cache, so
  this is the moment worth listening for.
- **Action failed.** A low, dark thump.

### The resolution

When the whole command finishes, the pad enters a closing phase. It holds
for a moment while the closing gesture swells in, then everything releases
together over several seconds. The pad and the gestures fade in one motion
so nothing is left ringing over silence.

- **Success.** A slowly-swelling BRAAAM at C1 lands underneath a rising
  major triad. The pad holds while the BRAAAM builds, then releases into
  it.
- **Cache-hit finish.** If a cache-hit bell chord rang in the last two
  seconds, the closing skips the BRAAAM entirely and lets the bells be the
  resolution. The reward moment gets to land instead of being buried.
- **Failure.** The chord shifts to the relative minor and the pad releases.

`once` waits for the release tail to finish before it lets the audio device
go, so the last few seconds of the piece always reach the speakers.

## Notes

- Sound is a runtime-only concern. The `--sound` flag never affects
  execution, caching, remote work, exit codes, or output. It is purely an
  additional channel for the person watching the run.
- The synth runs on its own thread and communicates with the CLI through a
  small lock-free control surface, so the audio path adds no measurable
  overhead to actions themselves.
- The tail wait can add up to about half a minute of process time on very
  short commands, because the closing arc is long by design. Drop the
  `--sound` flag when you need a quick exit.
