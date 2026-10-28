---
digest: In every bae app, current and future, a value that changes many times a second (playback position, transfer bytes, rates, levels) reaches the screen through a native view or layer that reads the stream's latest value per key once per display frame, never through the UI framework's observed state (SwiftUI, Compose, Avalonia bindings) and never with a main-thread hop per change.
paths:
  - '**/*.swift'
  - '**/*.kt'
  - '**/*.cs'
  - '**/*.axaml'
blocking: false
---

## High-frequency values skip the UI framework's observed state

Some values change many times a second: playback position, the bytes a file
upload or download has moved, transfer rates, meter levels. Core sends them on
keyed streams, not in tables. In every bae app, the ones that exist and the ones
still to come, they are drawn by a view the stream sets directly, not by the UI
framework's observed state.

Routing such a value through observed state (SwiftUI `@Observable`/`@State`,
Compose state read in composition, an Avalonia view-model property behind a
binding) re-runs the framework's update on every change: every view that reads
it re-evaluates, diffs and lays out, on the main thread, for each tick. Under a
fast upload that is hundreds of updates a second across every visible row.

Each app has two halves: the framework places the view once and hands it the
stream's key; the view reads the value itself. A new app follows the same split
from its first progress bar.

**Drawn once per frame, from the latest value per key.** A screen shows at most
one value per frame, and a transfer stream sends far more than that: a fast
upload is about 1,700 changes a second per stream. So the stream's callback does
not hop to the main thread with each change. It writes the latest value per key
into a thread-safe store on the thread it arrives on (core's stream already
merges changes the reader hasn't taken). The native view drawing a key reads
that key's latest value on the display's own frame callback while the key is
active, shows it if it changed, and stops when the key goes idle. Only a key
starting or stopping reaches the main thread, to start or stop the views drawing
it. That is the display's clock, not a timer standing in for knowing when a
value changed. In bae: `LatestByKey` and `FrameFollowing` (BaeKit, using an
`NSView` display link) and their Kotlin twins (`fm.bae.app.data`,
`fm.bae.app.ui.components`, using `Choreographer`).

**On macOS and iOS:** the AppKit/UIKit view (or `CALayer`) reads its key's
latest value each frame from its display link (`NSView.displayLink`,
`CADisplayLink`) and sets it: a bar's fraction, a label's text, a ring's
`strokeEnd`. SwiftUI places the view once through a representable and only
passes it the key's source; `updateNSView` handles configuration (colors, the
key the stream is for), never the value. `TransferRingNSView`,
`TransferReadoutNSView` and `StreamLabelNSView` are the pattern.

**On Android:** a Compose value read during composition recomposes on every
tick. Draw it in a View (through `AndroidView`) that reads the key's latest
value on a `Choreographer` frame callback while the key is active, or in a draw
lambda (`Modifier.drawBehind`, `graphicsLayer`) invalidated on the frame
callback.

**On Windows and Linux (Avalonia):** don't bind the value through a view model's
`INotifyPropertyChanged`. The control reads the key's latest value on the
compositor's frame callback (`TopLevel.RequestAnimationFrame`) while the key is
active and sets its own property, or draws it in its `Render` override and calls
`InvalidateVisual`.

**Which values are "high-frequency":** anything core sends faster than about
once a second while it is active. Counts that change when a state changes
(queued, stored, pinned) are state from tables and go through the framework like
any other row data.

**Smell:** a store or view-model property updated from a progress or position
stream and read in a SwiftUI `body`, a composable, or a binding; a
`Text("\(rate)")` fed from a throughput stream; a stream callback that hops to
the main thread (`Task { @MainActor in }`, `scope.launch(Dispatchers.Main)`) for
each value of a fast stream.
