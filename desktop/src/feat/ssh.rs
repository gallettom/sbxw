//! SSH access to sandboxes: the `*.sbx` hosts sbx writes into
//! `~/.ssh/config` (`sbx setup ssh`), and the one-click rewrite sbx 0.54
//! asks for (`assets/js/ssh.js`; `.sbxw-artifacts/reports/sbx-054.md`,
//! feature 5). Its card is drawn by Setup (`screens/setup.luau`, the
//! "Secrets and sign-in" tab); this feature only reads the state and runs
//! the rewrite, so the busy state and sbx's words on a failure are its own
//! (`on_answer`) while every other Setup dialog keeps its failures inside
//! the dialog.
//!
//! Data: `ssh` (`GET /api/ssh`: `{configured, refresh_due, sbx,
//! refreshed_at}`), read at connection and after the rewrite. A daemon
//! without the route answers 404: the card is then absent, never an error.
//! `ssh_run`: the rewrite under way (`{busy}`), done (`{done, refreshed_at}`)
//! or failed (`{error}`), view state of this run of the app.

use super::*;
use crate::intent::{Risk, Spec};
use serde_json::json;

pub const FEATURE: Feature = Feature {
    name: "ssh",
    data: &[Data::new("ssh", "/api/ssh")],
    intents: INTENTS,
    act,
    on_answer: Some(on_answer),
    standin: Some(standin),
    ..Feature::EMPTY
};

const INTENTS: &[Spec] = &[Spec {
    name: "ssh_setup",
    risk: Risk::Direct,
    args: &[],
    doc: "Rewrites the *.sbx block of ~/.ssh/config with the running sbx (`sbx setup ssh`), as sbx 0.54 asks once; the person's press in Setup's SSH card. Busy in `data.ssh_run` while it runs; sbx's words there if it fails.",
}];

/// Whether the daemon says the rewrite is due.
pub fn due(cx: &Ctx) -> bool {
    cx.get("ssh")["refresh_due"] == true || cx.get("daemon_state")["ssh_refresh_due"] == true
}

fn act(cx: &Ctx, a: &Asked) -> Result<Vec<Action>, String> {
    match a.name {
        "ssh_setup" => {
            if cx.get("ssh_run")["busy"] == true {
                return Err("the SSH configuration is being rewritten already".into());
            }
            Ok(vec![
                Action::Set { key: "ssh_run".into(), value: json!({ "busy": true }) },
                Action::Call(Call::post("/api/ssh/setup", json!({}), "rewrite the SSH configuration").tag("setup").timeout(120).then(&["ssh", "daemon_state"])),
            ])
        }
        n => Err(format!("{n}: not an ssh intent")),
    }
}

fn on_answer(_: &Ctx, _: &Call, r: Result<(u16, J), String>) -> Vec<Action> {
    match r {
        Ok((c, b)) if (200..300).contains(&c) && b["ok"] != false => {
            let with = b["refreshed_at"].as_str().map(|v| format!(" with sbx {v}")).unwrap_or_default();
            vec![
                Action::Set { key: "ssh_run".into(), value: json!({ "done": true, "refreshed_at": b["refreshed_at"] }) },
                Action::notice(format!("SSH configuration rewritten{with}")),
            ]
        }
        Ok((c, b)) => {
            let why = b["error"].as_str().map(str::to_string).unwrap_or_else(|| format!("the daemon answered {c}"));
            vec![Action::Set { key: "ssh_run".into(), value: json!({ "error": why }) }, Action::error(format!("Could not rewrite the SSH configuration: {why}"))]
        }
        Err(e) => vec![Action::Set { key: "ssh_run".into(), value: json!({ "error": e }) }, Action::error(format!("Could not rewrite the SSH configuration: {e}"))],
    }
}

// ---- stand-in routes (fixtures and src/bin/standin.rs) ----
//
// Fixture keys: `ssh` (the `GET /api/ssh` answer; none: set up with sbx
// 0.54, nothing due), `standin_ssh_fail` (a string: the rewrite fails with
// these words, once). A rewrite clears `refresh_due` in `ssh` and
// `daemon_state.ssh_refresh_due`.

fn standin(st: &mut Standin, method: &str, path: &str, _: &J) -> Option<(u16, J)> {
    match (method, path.split('?').next().unwrap_or("")) {
        ("GET", "/api/ssh") => {
            let v = st.data["ssh"].clone();
            Some((200, if v.is_null() { json!({ "ok": true, "configured": true, "refresh_due": false, "sbx": "0.54.0", "refreshed_at": "0.54.0" }) } else { v }))
        }
        ("POST", "/api/ssh/setup") => {
            if let Some(why) = st.data["standin_ssh_fail"].as_str().map(str::to_string) {
                st.data["standin_ssh_fail"] = J::Null;
                return Some((502, json!({ "ok": false, "error": why })));
            }
            let sbx = st.data["ssh"]["sbx"].as_str().unwrap_or("0.54.0").to_string();
            if st.data["ssh"].is_object() {
                st.data["ssh"]["refresh_due"] = false.into();
                st.data["ssh"]["configured"] = true.into();
                st.data["ssh"]["refreshed_at"] = sbx.clone().into();
            }
            if st.data["daemon_state"].is_object() {
                st.data["daemon_state"]["ssh_refresh_due"] = false.into();
            }
            Some((200, json!({ "ok": true, "refreshed_at": sbx, "refresh_due": false })))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(d: &mut J, name: &'static str) -> Vec<Action> {
        act(&Ctx { data: d }, &Asked { name, args: Default::default() }).unwrap()
    }

    fn served(d: &mut J, call: &Call) -> (u16, J) {
        let mut publish = |_: &str, _: J| {};
        let mut st = Standin { data: d, publish: &mut publish };
        standin(&mut st, "POST", &call.path, &call.body).unwrap()
    }

    fn set_of(v: &[Action], key: &str) -> J {
        v.iter().find_map(|a| match a {
            Action::Set { key: k, value } if k == key => Some(value.clone()),
            _ => None,
        }).unwrap_or(J::Null)
    }

    #[test]
    fn the_rewrite_is_busy_then_done_and_its_failure_is_sbxs_words() {
        let mut d = json!({ "ssh": { "ok": true, "configured": true, "refresh_due": true, "sbx": "0.54.1", "refreshed_at": "0.53.2" }, "daemon_state": { "ssh_refresh_due": true } });
        assert!(due(&Ctx { data: &d }));
        // A failure first: busy, then sbx's words.
        d["standin_ssh_fail"] = "sbx setup ssh: ~/.ssh/config is not writable".into();
        let v = run(&mut d, "ssh_setup");
        assert_eq!(set_of(&v, "ssh_run"), json!({ "busy": true }));
        let Some(Action::Call(call)) = v.iter().find(|a| matches!(a, Action::Call(_))) else { panic!() };
        assert_eq!((call.path.as_str(), call.then.clone()), ("/api/ssh/setup", vec!["ssh".to_string(), "daemon_state".to_string()]));
        d["ssh_run"] = json!({ "busy": true });
        assert!(act(&Ctx { data: &d }, &Asked { name: "ssh_setup", args: Default::default() }).is_err(), "one at a time");
        let r = served(&mut d, call);
        let a = on_answer(&Ctx { data: &d }, call, Ok(r));
        assert_eq!(set_of(&a, "ssh_run"), json!({ "error": "sbx setup ssh: ~/.ssh/config is not writable" }));
        assert!(due(&Ctx { data: &d }), "still due");
        // Again: done, and no longer due.
        d["ssh_run"] = J::Null;
        let v = run(&mut d, "ssh_setup");
        let Some(Action::Call(call)) = v.iter().find(|a| matches!(a, Action::Call(_))) else { panic!() };
        let r = served(&mut d, call);
        let a = on_answer(&Ctx { data: &d }, call, Ok(r));
        assert_eq!(set_of(&a, "ssh_run"), json!({ "done": true, "refreshed_at": "0.54.1" }));
        assert!(matches!(&a[1], Action::Notice { text, error: false, .. } if text == "SSH configuration rewritten with sbx 0.54.1"), "{a:?}");
        assert!(!due(&Ctx { data: &d }));
        assert_eq!(d["ssh"]["refreshed_at"], "0.54.1");
    }
}
