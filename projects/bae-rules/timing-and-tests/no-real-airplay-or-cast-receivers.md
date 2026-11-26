---
paths:
  - bae-cast/**
  - bae-bridge/**
  - bae-macos/**
  - bae-android/**
blocking: true
---

## No real AirPlay or Cast receivers

This rule is for agents that write or run bae's tests, probes and checks. A Mac
asks its owner whether to accept an AirPlay connection whenever something
connects to its AirPlay receiver, so a test or probe that dials this Mac's
receiver, or any receiver on the network, interrupts the person on their own
screen and casts to a device they did not pick.

No agent, test, benchmark or probe browses the network for receivers, connects
to a receiver, or casts to one. That includes a test marked `#[ignore]` and a
one-off probe an agent runs once.

- A test lists receivers through `RendererDiscovery::reported`, which takes the
  services the test hands it, and casts to bae-cast's fake receivers, which
  listen on `127.0.0.1`.
- A test never builds the mDNS daemon, and never builds
  `RendererDiscovery::builtin`, which browses the real network. Clippy refuses
  the daemon outside bae-cast's two discoveries (`clippy.toml`).
- A fake receiver that lacks a behavior gets that behavior added to the fake.
  The test never falls back to hardware.

A check that only real hardware can answer, such as whether a particular speaker
accepts bae's stream, is not an agent's check: it is a step for the person
(`builds help workflow ROLES`), which names the receiver and the steps.
