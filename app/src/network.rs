use warpui::{Entity, ModelContext, SingletonEntity};

/// Represents whether the client is connected to the network.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum NetworkStatusKind {
    /// The client is successfully connected to the network.
    /// When the app starts, we assume the client is connected to the network.
    /// If they actually weren't, we'd emit a network changed event immediately.
    /// If they were indeed connected, we _wouldn't_ emit an event (because there isn't a new status).
    #[default]
    Online,

    /// The client is not connected to the network.
    Offline,
}

/// Model that tracks the client's network connectivity status.
///
/// This model emits `NetworkStatusEvent::NetworkStatusChanged` when the network status changes.
pub struct NetworkStatus {
    status: NetworkStatusKind,
}

impl NetworkStatus {
    pub fn new() -> Self {
        Self {
            status: NetworkStatusKind::Online,
        }
    }

    /// Returns the current network status.
    pub fn status(&self) -> NetworkStatusKind {
        self.status
    }

    /// Returns true if the client is currently online.
    pub fn is_online(&self) -> bool {
        self.status == NetworkStatusKind::Online
    }

    pub fn reachability_changed(&mut self, reachable: bool, ctx: &mut ModelContext<Self>) {
        let old_status = self.status;
        let new_status = if reachable {
            NetworkStatusKind::Online
        } else {
            NetworkStatusKind::Offline
        };

        if old_status != new_status {
            self.status = new_status;

            ctx.emit(NetworkStatusEvent::NetworkStatusChanged { new_status });
        }
    }
}

impl Default for NetworkStatus {
    fn default() -> Self {
        Self::new()
    }
}

pub enum NetworkStatusEvent {
    NetworkStatusChanged { new_status: NetworkStatusKind },
}

impl Entity for NetworkStatus {
    type Event = NetworkStatusEvent;
}

impl SingletonEntity for NetworkStatus {}
