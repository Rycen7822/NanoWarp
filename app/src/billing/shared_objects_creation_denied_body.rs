use warpui::elements::{Container, CrossAxisAlignment, Flex, ParentElement, Text};
use warpui::{AppContext, Element, Entity, SingletonEntity, View, ViewContext};

use crate::appearance::Appearance;
use crate::drive::DriveObjectType;
use crate::ui_components::blended_colors;
use crate::workspaces::workspace::{BillingMetadata, CustomerType};

const DEFAULT_DELINQUENT_ADMIN_MODAL_SUBHEADER: &str = "Shared drive objects have been restricted due to a subscription payment issue.\n\nPlease update your payment information to restore access.";
const DEFAULT_DELINQUENT_ADMIN_ENTERPRISE_MODAL_SUBHEADER: &str = "Shared drive objects have been restricted due to a subscription payment issue.\n\nPlease contact support@warp.dev to restore access.";
const DEFAULT_DELINQUENT_MODAL_SUBHEADER: &str = "Shared drive objects have been restricted due to a subscription payment issue.\n\nPlease contact a team admin to restore access.";
const DEFAULT_ADMIN_PROSUMER_MODAL_SUBHEADER: &str = "Warp's Pro plan comes with a limited number of shared drive objects.\n\nFor access to unlimited shared drive objects, upgrade to the Turbo plan.";
const DEFAULT_PROSUMER_MODAL_SUBHEADER: &str = "Warp's Pro plan comes with a limited number of shared drive objects.\n\nFor access to unlimited shared drive objects, contact a team admin to upgrade to the Turbo plan.";
const DEFAULT_ADMIN_MODAL_SUBHEADER: &str = "Warp's free plan comes with a limited number of shared drive objects.\n\nFor access to unlimited shared drive objects, upgrade to a paid plan.";
const DEFAULT_MODAL_SUBHEADER: &str = "Warp's free plan comes with a limited number of shared drive objects.\n\nFor access to unlimited shared drive objects, contact a team admin to upgrade to a paid plan.";
pub struct SharedObjectsCreationDeniedBody {
    object_type: Option<DriveObjectType>,
    has_admin_permissions: bool,
    is_delinquent_due_to_payment_issue: bool,
    customer_type: CustomerType,
}

impl SharedObjectsCreationDeniedBody {
    pub fn new(object_type: Option<DriveObjectType>) -> Self {
        Self {
            object_type,
            has_admin_permissions: false,
            is_delinquent_due_to_payment_issue: false,
            customer_type: Default::default(),
        }
    }

    pub fn update_state(
        &mut self,
        object_type: DriveObjectType,
        has_admin_permissions: bool,
        is_delinquent_due_to_payment_issue: bool,
        customer_type: CustomerType,
        ctx: &mut ViewContext<Self>,
    ) {
        self.object_type = Some(object_type);
        self.has_admin_permissions = has_admin_permissions;
        self.is_delinquent_due_to_payment_issue = is_delinquent_due_to_payment_issue;
        self.customer_type = customer_type;
        ctx.notify();
    }
}

impl Entity for SharedObjectsCreationDeniedBody {
    type Event = ();
}

impl View for SharedObjectsCreationDeniedBody {
    fn ui_name() -> &'static str {
        "SharedObjectsCreationDeniedBody"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let is_stripe_paid_plan = BillingMetadata::is_stripe_paid_plan(self.customer_type);

        let sub_header = match self.object_type {
            Some(object_type) => {
                match (
                    self.is_delinquent_due_to_payment_issue,
                    self.has_admin_permissions,
                    self.customer_type,
                ) {
                    (true, true, _) => {
                        if is_stripe_paid_plan {
                            format!(
                                "Shared {object_type}s have been restricted due to a subscription payment issue.\n\nPlease update your payment information to restore access."
                            )
                        } else {
                            format!(
                                "Shared {object_type}s have been restricted due to a subscription payment issue.\n\nPlease contact support@warp.dev to restore access."
                            )
                        }
                    }
                    (true, false, _) => format!(
                        "Shared {object_type}s have been restricted due to a subscription payment issue.\n\nPlease contact a team admin to restore access."
                    ),
                    (false, true, CustomerType::Prosumer) => {
                        format!(
                            "Warp's Pro plan comes with a limited number of shared {object_type}s.\n\nFor access to unlimited shared {object_type}s, upgrade to the Build plan."
                        )
                    }
                    (false, false, CustomerType::Prosumer) => {
                        format!(
                            "Warp's Pro plan comes with a limited number of shared {object_type}s.\n\nFor access to unlimited shared {object_type}s, contact a team admin to upgrade to the Build plan."
                        )
                    }
                    (false, true, _) => format!(
                        "Warp's free plan comes with a limited number of shared {object_type}s.\n\nFor access to unlimited shared {object_type}s, upgrade to a paid plan."
                    ),
                    (false, false, _) => format!(
                        "Warp's free plan comes with a limited number of shared {object_type}s.\n\nFor access to unlimited shared {object_type}s, contact a team admin to upgrade to a paid plan."
                    ),
                }
            }
            _ => match (
                self.is_delinquent_due_to_payment_issue,
                self.has_admin_permissions,
                self.customer_type,
            ) {
                (true, true, _) => {
                    if is_stripe_paid_plan {
                        DEFAULT_DELINQUENT_ADMIN_MODAL_SUBHEADER.into()
                    } else {
                        DEFAULT_DELINQUENT_ADMIN_ENTERPRISE_MODAL_SUBHEADER.into()
                    }
                }
                (true, false, _) => DEFAULT_DELINQUENT_MODAL_SUBHEADER.into(),
                (false, true, CustomerType::Prosumer) => {
                    DEFAULT_ADMIN_PROSUMER_MODAL_SUBHEADER.into()
                }
                (false, false, CustomerType::Prosumer) => DEFAULT_PROSUMER_MODAL_SUBHEADER.into(),
                (false, true, _) => DEFAULT_ADMIN_MODAL_SUBHEADER.into(),
                (false, false, _) => DEFAULT_MODAL_SUBHEADER.into(),
            },
        };

        let body = Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(
                    Text::new(sub_header, appearance.ui_font_family(), 14.)
                        .with_color(blended_colors::text_sub(
                            appearance.theme(),
                            appearance.theme().background(),
                        ))
                        .finish(),
                )
                .finish(),
            );

        body.finish()
    }
}
