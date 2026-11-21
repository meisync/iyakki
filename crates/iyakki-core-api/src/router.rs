//! The model router's door to the core (F-ROUTER-06; the owner's core rule: nothing that decides which model sees a person's work is
//! reached around the core). The router itself (plans, strategies, model calls) lives in `iyakki-agent`; what it must ask the core is small:
//!
//! * `router.use` (a hard ask, a card every time) before a run starts: a run sends the person's goal and what the steps produce to the
//!   models the person allowed, and spends tokens;
//! * `router.strategy.set` (a hard ask) before the strategy that decides which model takes which step is changed or a strategy script is added;
//! * one access-log line for every decision of a run (`record`): ids and numbers only, never message content.
//!
//! The device gives a run `iyakki_core_local::LocalRouter` (the broker has already decided; the line goes to the guard), a test gives
//! `iyakki_core_fake::FakeRouter`. A machine whose `Os` does not say otherwise has no router and refuses ([`NoRouter`]).

pub trait Router: Send + Sync {
    /// Permission to start a run; `label` says what (the number of steps, the strategy), never the goal's words.
    fn begin_run(&self, label: &str) -> Result<(), String>;
    /// Permission to listen through the microphone for one step (`microphone.listen`, a card every time; F-ROUTER-11). `label` says how long,
    /// never what is wanted. The default refuses.
    fn allow_listen(&self, _label: &str) -> Result<(), String> {
        Err("nobody can be asked on this machine, so a step cannot listen".into())
    }
    /// Permission for ONE tool call of a step that runs unattended (F-ROUTER-40): a card on the screen, the step waits for the answer. `label`
    /// names the tool and what it would do, never the goal. The default refuses: a machine with nobody to ask gives no permission.
    fn ask_step_tool(&self, _label: &str) -> Result<(), String> {
        Err("nobody can be asked on this machine, so a step cannot use a tool that asks".into())
    }
    /// Permission to choose a strategy or add a strategy script.
    fn allow_strategy(&self, name: &str) -> Result<(), String>;
    /// One decision of a run for the access log (`what` like `router.assign`, `detail` ids and numbers).
    fn record(&self, what: &str, detail: &str);
    /// The models the person can use, one profile each (read only; no key and no address in it). The default is an empty list: a machine
    /// with no registry offers no model (F-ROUTER-01).
    fn models(&self) -> Vec<RouteModel> {
        Vec::new()
    }
    /// The routing decision for one request, as a question and not an act: which models fit `need`, best first, and why each of the others
    /// does not. Nothing is sent, nothing is spent, nothing is written. Built on [`Router::models`] and [`choose`].
    fn route(&self, need: &RouteNeed) -> RouteDecision {
        choose(&self.models(), need)
    }
}

/// How a model is paid for, from the cheapest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteCost {
    Free,
    Low,
    Medium,
    High,
}

/// Where a model runs, from the most private.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteZone {
    Computer,
    Network,
    Internet,
}

/// One model as the routing decision sees it. Plain data: it is the same for a model on this computer, on the network, on the internet or
/// on a fleet member.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RouteModel {
    /// `provider/model`.
    pub id: String,
    /// `planner`, `executor` or `both`.
    pub role: String,
    pub tools: bool,
    pub vision: bool,
    pub audio: bool,
    /// Context size in tokens.
    pub context: u32,
    pub cost: RouteCost,
    pub zone: RouteZone,
    pub enabled: bool,
    /// Mean milliseconds per answer when known (the usage ledger), for a tie-break.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_ms: Option<u64>,
}

/// What a request needs. Every field is a limit; `Default` asks for nothing special.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RouteNeed {
    pub tools: bool,
    pub vision: bool,
    pub audio: bool,
    /// Smallest context size wanted, in tokens.
    pub min_context: u32,
    /// Dearest cost class allowed (none: any).
    pub max_cost: Option<RouteCost>,
    /// Farthest zone allowed (none: any).
    pub max_zone: Option<RouteZone>,
    /// The request is for a plan: only models with the planner role (`planner` or `both`) fit. Otherwise `executor` or `both`.
    pub planning: bool,
}

/// The answer to a [`RouteNeed`].
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RouteDecision {
    /// The model the router would use first, when any fits.
    pub chosen: Option<String>,
    /// Every model that fits, best first (the chosen one is the first).
    pub ranked: Vec<String>,
    /// Every model that does not fit, with the plain reason (`id`, reason), in registry order.
    pub refused: Vec<(String, String)>,
}

/// The built-in order, with no side effect: the most private zone first, then the cheaper cost, then the faster known answer, then the larger
/// context, then the name (so the answer never depends on the order of the list).
pub fn choose(models: &[RouteModel], need: &RouteNeed) -> RouteDecision {
    let mut fit: Vec<&RouteModel> = Vec::new();
    let mut refused = Vec::new();
    for m in models {
        let why = if !m.enabled {
            Some("it is switched off")
        } else if need.planning && m.role == "executor" {
            Some("it only runs steps, it does not plan")
        } else if !need.planning && m.role == "planner" {
            Some("it only plans, it does not run steps")
        } else if need.tools && !m.tools {
            Some("it cannot call tools")
        } else if need.vision && !m.vision {
            Some("it cannot read images")
        } else if need.audio && !m.audio {
            Some("it cannot take sound")
        } else if m.context < need.min_context {
            Some("its context is too small")
        } else if need.max_cost.is_some_and(|c| m.cost > c) {
            Some("it costs more than allowed")
        } else if need.max_zone.is_some_and(|z| m.zone > z) {
            Some("it runs farther away than allowed")
        } else {
            None
        };
        match why {
            Some(w) => refused.push((m.id.clone(), w.to_string())),
            None => fit.push(m),
        }
    }
    fit.sort_by(|a, b| (a.zone, a.cost, a.answer_ms.unwrap_or(u64::MAX), std::cmp::Reverse(a.context), &a.id).cmp(&(b.zone, b.cost, b.answer_ms.unwrap_or(u64::MAX), std::cmp::Reverse(b.context), &b.id)));
    let ranked: Vec<String> = fit.iter().map(|m| m.id.clone()).collect();
    RouteDecision { chosen: ranked.first().cloned(), ranked, refused }
}

#[cfg(test)]
mod route_tests {
    use super::*;

    fn m(id: &str, zone: RouteZone, cost: RouteCost) -> RouteModel {
        RouteModel { id: id.into(), role: "both".into(), tools: true, vision: false, audio: false, context: 8192, cost, zone, enabled: true, answer_ms: None }
    }

    #[test]
    fn the_most_private_then_the_cheapest_model_that_fits_is_chosen_and_the_others_say_why_not() {
        let mut big = m("net/big", RouteZone::Network, RouteCost::Free);
        big.vision = true;
        let mut off = m("x/off", RouteZone::Computer, RouteCost::Free);
        off.enabled = false;
        let list = vec![m("cloud/a", RouteZone::Internet, RouteCost::Low), big, m("local/s", RouteZone::Computer, RouteCost::Free), off];
        let d = choose(&list, &RouteNeed::default());
        assert_eq!(d.chosen.as_deref(), Some("local/s"));
        assert_eq!(d.ranked, ["local/s", "net/big", "cloud/a"]);
        assert_eq!(d.refused, [("x/off".to_string(), "it is switched off".to_string())]);
        let v = choose(&list, &RouteNeed { vision: true, ..Default::default() });
        assert_eq!(v.ranked, ["net/big"]);
        assert!(v.refused.iter().any(|(i, w)| i == "local/s" && w == "it cannot read images"));
        let near = choose(&list, &RouteNeed { max_zone: Some(RouteZone::Network), ..Default::default() });
        assert!(near.refused.iter().any(|(i, w)| i == "cloud/a" && w.contains("farther")));
    }

    #[test]
    fn roles_context_and_cost_limits_apply_and_the_order_does_not_depend_on_the_list_order() {
        let mut planner = m("a/plan", RouteZone::Computer, RouteCost::Free);
        planner.role = "planner".into();
        let mut small = m("a/small", RouteZone::Computer, RouteCost::Free);
        small.context = 2048;
        let dear = m("b/dear", RouteZone::Computer, RouteCost::High);
        let list = vec![planner, small, dear];
        assert_eq!(choose(&list, &RouteNeed { planning: true, ..Default::default() }).ranked, ["a/plan", "a/small", "b/dear"]);
        let run = choose(&list, &RouteNeed { min_context: 4096, max_cost: Some(RouteCost::Medium), ..Default::default() });
        assert!(run.ranked.is_empty() && run.chosen.is_none() && run.refused.len() == 3, "{run:?}");
        let mut rev = list.clone();
        rev.reverse();
        assert_eq!(choose(&list, &RouteNeed::default()), choose(&rev, &RouteNeed::default()));
    }

    #[test]
    fn a_machine_with_no_registry_offers_nothing_and_a_known_speed_breaks_a_tie() {
        assert_eq!(NoRouter.route(&RouteNeed::default()), RouteDecision::default());
        let mut a = m("a/x", RouteZone::Computer, RouteCost::Free);
        a.answer_ms = Some(900);
        let mut b = m("b/y", RouteZone::Computer, RouteCost::Free);
        b.answer_ms = Some(300);
        assert_eq!(choose(&[a, b], &RouteNeed::default()).chosen.as_deref(), Some("b/y"));
    }
}

/// A machine without a router: every request is refused and nothing is written.
pub struct NoRouter;

impl Router for NoRouter {
    fn begin_run(&self, _: &str) -> Result<(), String> {
        Err("this machine has no model router".into())
    }
    fn allow_strategy(&self, _: &str) -> Result<(), String> {
        Err("this machine has no model router".into())
    }
    fn record(&self, _: &str, _: &str) {}
}
