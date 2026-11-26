/// How long a wait goes on for something it knows is coming before it calls
/// the hang a failure: the one limit of every `Wait`.
///
/// Every wait it bounds ends on its own event, as soon as the event comes,
/// so it paces nothing and sits far above what any step takes on a loaded
/// machine.
enum HangGuard {
    static let duration: Duration = .seconds(45)
}
