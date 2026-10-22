use serde_json::json;
use warp_core::telemetry::TelemetryEvent;

use super::{PricingPromotionSurface, PricingPromotionTelemetryEvent};

#[test]
fn promotion_telemetry_payload_includes_surface() {
    for (event, surface) in [
        (
            PricingPromotionTelemetryEvent::Shown {
                surface: PricingPromotionSurface::AgentMessageBar,
            },
            "agent_message_bar",
        ),
        (
            PricingPromotionTelemetryEvent::Clicked {
                surface: PricingPromotionSurface::AgentMessageBar,
            },
            "agent_message_bar",
        ),
        (
            PricingPromotionTelemetryEvent::Dismissed {
                surface: PricingPromotionSurface::AgentMessageBar,
            },
            "agent_message_bar",
        ),
    ] {
        assert_eq!(
            event.payload(),
            Some(json!({
                "surface": surface,
            }))
        );
    }
}
