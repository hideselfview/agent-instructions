---
paths:
  - design/**
  - bae-macos/**
  - BaeKit/**
  - bae-android/**
  - bae-bridge/loc/catalog.toml
  - plans/**
blocking: true
design: true
---

## Storage and pinning words

- The person's cloud storage is **the cloud** in everyday words, on every
  platform and screen: the Storage Manager's "Cloud" chip and filter, "Move to
  cloud", "Cloud full", "Could not reach the cloud", "The library will remain in
  the cloud".
  - Where the person sets up sync, the place itself (the folder in their
    Dropbox, OneDrive or iCloud, or the S3 storage in their S3 service, that the
    library lives in: coven's *store* on its *provider*) is their **cloud
    home**: the Sync settings section headed "Cloud home", "Choose a cloud
    home", setup's steps.
  - Never write "storage" alone for either: people can't tell which storage.
    Storage Manager and a release's Storage sheet keep "Storage", since they
    cover the files on this device too, and so does a phone's own storage.
- Keeping a release's files on a device is pinning, on every platform and every
  screen: the action is Pin; under way, Pinning; done, Pinned; undone, Unpin;
  stopped while under way, Cancel pinning; waiting, Queued to pin; failed, Could
  not pin. Never write "download", "keep here", "copying here" or "for offline"
  for it. The screen of pins on Android is Pinning; its settings are Pins at
  once, Pause pinning and Pin over Wi-Fi only.
- Moving a release to This Mac is a different thing with its own words, named
  for the device as the import's destination names it (This Mac or Cloud): Move
  to This Mac…; under way, Moving to This Mac…; waiting, Queued to move to This
  Mac; failed, Could not move to This Mac. Never write "make local" for it, and
  never count, show or name it as pinning: a pin keeps a copy of a cloud file in
  this device's cache and changes nothing elsewhere, while moving to This Mac
  changes where the file lives, for every device.
- On desktop pinning and moving share one download queue, so they contend for
  the same downloads; what covers both says downloads: Pause downloads,
  Downloads at once, the footer's ↓ rate.
- A waiting state names what it waits to do: Queued to pin, Queued to upload,
  Queued to move to This Mac. The one exception is the Storage Manager's Storage
  column, where each state is one word: every state that stands still is a plain
  word (Local, Pinned, Cloud, Queued, and Waiting and Error in their tone's
  colour), and only Uploading, Pinning and Moving, which fill with progress, are
  chips. The inspector says the rest: what is queued or moving, what failed, and
  what an upload waits for.
- A release's storage shows its state and at most one pill for the change on its
  way; nothing says how many wait ahead of it. What holds a whole queue back
  (Paused, Waiting for network, Waiting for Wi-Fi, Retrying at…) shows once, in
  words, where the queue shows: the Storage Manager's footer, the Activity
  panel's transfer lines, Android's Pinning screen. A release's own problem,
  such as a missing original that stops its upload, shows on the release.
