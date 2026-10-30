//! The assistant's tools for platform services. Reading is free (`platform_list`, `platform_status`, `git_service_status`); `platform_deploy`,
//! `platform_upgrade` and `platform_remove` ask in every approval mode (`agent::hard_ask`), show the exact plan and name the cluster by
//! server. The cluster must be one of the ids `platform_list` gave (a cluster the person can see, not hidden, not production-looking, not
//! read-only); the assistant cannot invent one. No tool result ever holds a password or token.

use serde_json::{json, Value};

use super::*;
use crate::agent::{Risk, ToolDef};
use crate::chat::ToolSpec;

pub const TOOLS: [&str; 6] = ["platform_list", "platform_status", "platform_deploy", "platform_upgrade", "platform_remove", "git_service_status"];

pub fn handles(name: &str) -> bool {
    TOOLS.contains(&name)
}

fn arg<'a>(args: &'a Value, k: &str) -> &'a str {
    args.get(k).and_then(Value::as_str).map(str::trim).unwrap_or("")
}

pub struct PlatformTools {
    pub pl: Arc<Platform>,
}

impl PlatformTools {
    pub fn new(pl: Arc<Platform>) -> PlatformTools {
        PlatformTools { pl }
    }

    pub fn defs(&self) -> Vec<ToolDef> {
        let obj = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
        let t = |name: &str, desc: &str, schema: Value, risk: Risk, untrusted: bool| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted };
        vec![
            t("platform_list", "Read-only: the platform services IYAKKI can deploy into a Kubernetes cluster (Gitea, a private store, ...) with their pinned versions and options, the clusters the person can use (each with an `id`, its server and why it is refused, if it is), and the services already installed (with their instance ids and state). Call it first: platform_deploy needs a cluster id from here. Never pick a cluster the person did not ask for.", obj(json!({}), &[]), Risk::Safe, false),
            t("platform_status", "Read-only: the live state of an installed service by `instance` (from platform_list): running, stopped, degraded, ready replicas, the running image and whether its address answers. No `instance`: every installed service.", obj(json!({"instance": {"type": "string"}}), &[]), Risk::Safe, true),
            t("platform_deploy", "Deploy a platform service into a cluster, after the person reviews the exact plan (manifests, images, ports, storage, the credentials it creates, what it exposes) and approves; it ASKS in every mode. `service` (id from platform_list), `profile` (a cluster id from platform_list, one the person named: never choose one yourself), optional `namespace` (default iyakki-<service>; a new one is created and labelled, an existing one must already be labelled managed-by=iyakki) and `options` (object: version, storage_size, storage_class, exposure ClusterIP|NodePort|LoadBalancer|Ingress, ingress_host, admin_user, mirror, and the service's own). Production-looking, read-only, hidden and unreachable clusters are refused. Generated passwords go to a Kubernetes Secret and the person's vault; you never see them.", obj(json!({"service": {"type": "string"}, "profile": {"type": "string"}, "namespace": {"type": "string"}, "options": {"type": "object"}}), &["service", "profile"]), Risk::Ask, false),
            t("platform_upgrade", "Upgrade an installed service to a newer pinned version along its upgrade path: `instance`, optional `version` (default the newest known), `backup_first` (default true: a dump is taken and a failure stops the upgrade). The person reviews the plan naming the cluster and approves; it ASKS in every mode and is never run automatically.", obj(json!({"instance": {"type": "string"}, "version": {"type": "string"}, "backup_first": {"type": "boolean"}}), &["instance"]), Risk::Ask, false),
            t("platform_remove", "Remove an installed service: `instance`, `delete_data` (default false: the PVC, Secrets, vault entries and namespace are KEPT), and `confirm`, which must be the instance id typed by the person (ask them with ask_user; never fill it in). It ASKS in every mode and the card says exactly what is deleted.", obj(json!({"instance": {"type": "string"}, "delete_data": {"type": "boolean"}, "confirm": {"type": "string"}}), &["instance", "confirm"]), Risk::Ask, false),
            t("git_service_status", "Read-only: the installed git services (Gitea) with their live state, web and clone addresses (no tokens), and the workspaces connected to each. This includes the Gitea the person installed from Store \u{2192} Setup (\"Gitea (versioning for your work)\", reachable on this machine at 127.0.0.1 and added as the Git connection \"Local Gitea\").", obj(json!({}), &[]), Risk::Safe, true),
        ]
    }

    fn installed(&self, id: &str) -> Result<Installed, String> {
        installed::find(&self.pl.dir, &self.pl.user, id).ok_or_else(|| format!("`{id}` is not an installed service (platform_list shows the instance ids)"))
    }

    pub fn describe(&self, name: &str, args: &Value) -> String {
        let cluster = |id: &str| match cached_profile(&self.pl.dir, &self.pl.user, id) {
            Some((n, server, None)) => format!("the cluster \"{n}\" at {server}"),
            Some((n, server, Some(_))) => format!("the cluster \"{n}\" at {server} \u{2014} will be REFUSED"),
            None => format!("the cluster `{id}` \u{2014} will be REFUSED: not one platform_list gave"),
        };
        match name {
            "platform_list" => "list the platform services, clusters and installed services".into(),
            "platform_status" | "git_service_status" => "read the state of the installed services".into(),
            "platform_deploy" => format!("DEPLOY {} to {}, namespace {} (you review the exact plan first)", spec(arg(args, "service")).map(|d| d.title()).unwrap_or_else(|| format!("`{}`", arg(args, "service"))), cluster(arg(args, "profile")), if arg(args, "namespace").is_empty() { "the default".to_string() } else { arg(args, "namespace").to_string() }),
            "platform_upgrade" => match self.installed(arg(args, "instance")) {
                Ok(i) => format!("UPGRADE {} {} on {} to {}{}", i.title, i.version, i.cluster.as_ref().map(ClusterRef::line).unwrap_or_else(|| "its service".into()), if arg(args, "version").is_empty() { "the newest known version".to_string() } else { arg(args, "version").to_string() }, if args.get("backup_first").and_then(Value::as_bool) == Some(false) { " WITHOUT a backup" } else { " (backup first)" }),
                Err(e) => format!("upgrade `{}` ({e})", arg(args, "instance")),
            },
            "platform_remove" => match self.installed(arg(args, "instance")) {
                Ok(i) => format!("REMOVE {} from {}{} (type the id `{}` to confirm)", i.title, i.cluster.as_ref().map(ClusterRef::line).unwrap_or_else(|| "its record".into()), if args.get("delete_data").and_then(Value::as_bool) == Some(true) { " AND DELETE ITS DATA" } else { ", keeping its data" }, i.id),
                Err(e) => format!("remove `{}` ({e})", arg(args, "instance")),
            },
            _ => name.into(),
        }
    }

    pub fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            "platform_deploy" => Some(match self.plan_from(args) {
                Ok(p) => preview(&p),
                Err(e) => format!("this would be refused: {e}"),
            }),
            "platform_upgrade" => Some(match self.upgrade_plan_from(args) {
                Ok(u) => preview_upgrade(&u),
                Err(e) => format!("this would be refused: {e}"),
            }),
            "platform_remove" => Some(match self.installed(arg(args, "instance")) {
                Ok(i) => preview_remove(&plan_remove(&i, args.get("delete_data").and_then(Value::as_bool).unwrap_or(false))),
                Err(e) => format!("this would be refused: {e}"),
            }),
            _ => None,
        }
    }

    fn plan_from(&self, args: &Value) -> Result<Plan, String> {
        let def = spec(arg(args, "service")).ok_or_else(|| format!("`{}` is not a platform service (platform_list shows them)", arg(args, "service")))?;
        let profile = profile_by_id(&self.pl, arg(args, "profile"))?;
        let mut opts = Options::from_json(args.get("options").unwrap_or(&Value::Null));
        if !arg(args, "namespace").is_empty() {
            opts.set("namespace", arg(args, "namespace"));
        }
        plan(&self.pl, &*def, &profile, &opts)
    }

    fn upgrade_plan_from(&self, args: &Value) -> Result<UpgradePlan, String> {
        let i = self.installed(arg(args, "instance"))?;
        plan_upgrade(&self.pl, &i, Some(arg(args, "version")), args.get("backup_first").and_then(Value::as_bool).unwrap_or(true))
    }

    pub fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "platform_list" => Ok(self.list()),
            "platform_status" => self.status_text(arg(args, "instance")),
            "git_service_status" => Ok(self.git_status()),
            "platform_deploy" => {
                let p = self.plan_from(args)?;
                if !p.can_apply() {
                    return Err(format!("not deployed: {}", p.blockers.join("; ")));
                }
                let a = apply(&self.pl, &p, true, &mut |_| {})?;
                let mut t = format!("{} {} on {}\n", p.title, if a.ready { "is running" } else { "was applied but is not ready yet" }, p.cluster.line());
                for d in &a.done {
                    t.push_str(&format!("- {d}\n"));
                }
                t.push_str("\nAddresses:\n");
                for u in &a.urls {
                    t.push_str(&format!("- {}: {}\n", u.label, u.url));
                }
                let names: Vec<String> = p.secrets.iter().flat_map(|s| s.entries.iter().map(|e| format!("uvault:{}", e.vault))).collect();
                t.push_str(&format!("\nThe generated credentials are in the person's vault ({}) and in the Kubernetes Secret; they are not shown to you. The person can reveal them in Settings, Dev cluster, Platform services, Details.\n\n{}\nInstance id: {}", names.join(", "), a.how_to_use, a.instance));
                Ok(t)
            }
            "platform_upgrade" => {
                let u = self.upgrade_plan_from(args)?;
                let done = upgrade(&self.pl, &u, true, &mut |_| {})?;
                Ok(format!("Upgraded {} from {} to {}:\n- {}", u.inst.title, u.from, u.to, done.join("\n- ")))
            }
            "platform_remove" => {
                let i = self.installed(arg(args, "instance"))?;
                let r = plan_remove(&i, args.get("delete_data").and_then(Value::as_bool).unwrap_or(false));
                let done = remove(&self.pl, &r, arg(args, "confirm"), true)?;
                Ok(format!("Removed {}:\n- {}", i.title, done.join("\n- ")))
            }
            _ => Err(format!("unknown tool {name}")),
        }
    }

    fn list(&self) -> String {
        let mut t = String::from("SERVICES\n");
        for d in specs() {
            t.push_str(&format!("- {} ({}, {}): {}\n  versions: {} (default {}); options: {}\n", d.id(), d.title(), d.kind(), d.description(), d.versions().join(", "), d.versions().last().cloned().unwrap_or_default(), common_options(&*d).iter().map(|o| o.key.as_str()).collect::<Vec<_>>().join(", ")));
        }
        t.push_str("\nCLUSTERS (use the id as `profile`; refused ones cannot be used)\n");
        let l = list_profiles(&self.pl);
        if l.is_empty() {
            t.push_str("- none found\n");
        }
        for c in &l {
            t.push_str(&format!("- {} \u{2014} {} at {} ({}){}\n", c.id, if c.profile.name.is_empty() { "cluster" } else { &c.profile.name }, c.profile.server, c.profile.state(), c.refusal.as_ref().map(|r| format!("; {r}")).unwrap_or_default()));
        }
        t.push_str("\nINSTALLED\n");
        let inst = installed::load(&self.pl.dir, &self.pl.user);
        if inst.is_empty() {
            t.push_str("- none\n");
        }
        for i in &inst {
            t.push_str(&format!("- {} \u{2014} {} {} in {} on {}{}\n", i.id, i.title, i.version, i.namespace, i.cluster.as_ref().map(ClusterRef::line).unwrap_or_else(|| "(connected)".into()), if i.connected { " [connected, not deployed by IYAKKI]" } else { "" }));
        }
        t.trim_end().to_string()
    }

    fn status_text(&self, id: &str) -> Result<String, String> {
        let all = installed::load(&self.pl.dir, &self.pl.user);
        let sel: Vec<&Installed> = if id.is_empty() { all.iter().collect() } else { vec![all.iter().find(|i| i.id == id).ok_or_else(|| format!("`{id}` is not an installed service"))?] };
        if sel.is_empty() {
            return Ok("No platform service is installed.".into());
        }
        let mut t = String::new();
        for i in sel {
            let s = status(&self.pl, i, true);
            t.push_str(&format!("{} ({}) {} {}: {}{}\n", i.id, i.title, i.version, i.web_url().unwrap_or(""), s.line(), s.reachable.map(|r| if r { ", the address answers" } else { ", the address does not answer from here" }).unwrap_or("")));
            for n in &s.notes {
                t.push_str(&format!("  note: {n}\n"));
            }
        }
        Ok(t.trim_end().to_string())
    }

    fn git_status(&self) -> String {
        let git: Vec<Installed> = installed::load(&self.pl.dir, &self.pl.user).into_iter().filter(|i| i.kind == "git").collect();
        if git.is_empty() {
            return "No git service is installed or connected (Settings, Dev cluster, Platform services, or the IDE's K8S panel, Services).".into();
        }
        let mut t = String::new();
        for i in &git {
            let s = status(&self.pl, i, true);
            t.push_str(&format!("{} \u{2014} {} {}: {}\n", i.id, i.title, i.version, s.line()));
            for u in &i.urls {
                t.push_str(&format!("  {}: {}\n", u.label, u.url));
            }
            for w in &i.workspaces {
                t.push_str(&format!("  workspace {} \u{2192} {}\n", w.workspace, w.repo_url));
            }
        }
        t.trim_end().to_string()
    }
}
