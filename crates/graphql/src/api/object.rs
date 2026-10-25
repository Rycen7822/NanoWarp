use crate::schema;

#[derive(cynic::QueryFragment, Debug, Clone)]
pub struct Space {
    pub uid: cynic::Id,
    #[cynic(rename = "type")]
    pub type_: SpaceType,
}

#[derive(cynic::Enum, Clone, Copy, Debug)]
pub enum SpaceType {
    #[cynic(rename = "Team")]
    Team,
    #[cynic(rename = "User")]
    User,
}
