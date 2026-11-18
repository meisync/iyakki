use crate::cloudnames::plain_name;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Tenant,
    Namespace(String),
    Common,
}

impl Scope {
    pub fn parse(s: &str) -> Option<Scope> {
        match s {
            "tenant" => Some(Scope::Tenant),
            "common" => Some(Scope::Common),
            _ => s.strip_prefix("namespace:").filter(|n| plain_name(n)).map(|n| Scope::Namespace(n.into())),
        }
    }

    pub fn name(&self) -> String {
        match self {
            Scope::Tenant => "tenant".into(),
            Scope::Namespace(n) => format!("namespace:{n}"),
            Scope::Common => "common".into(),
        }
    }
}
