//! 67 §4.4: the roster follows the mesh lease with a [`Follower`] (the pure
//! state machine lives in `mesh::lease` since 69 so `link` can follow the
//! lease without depending on roster). 182 D4: desks no longer move with it —
//! only the legacy (NULL-home) tasks and the `DeliverWake` backstop read it.

pub use mesh::lease::Follower;
