//! Types moved out of `iyakki_agent::routertools` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



pub const PROMPT_RULE: &str = "\nMODEL ROUTER: the router lets a bigger model plan and lighter models do the steps. router_status says whether it is on and what it used; router_models lists each model with its role, abilities, cost and where it runs; router_strategy_list lists the strategies. router_status also shows a run in progress; router_cancel stops it after the model call that is running. Starting a run (router_run) and changing anything (router_set, router_strategy_set, router_strategy_add, router_strategy_remove) asks the person every time; router_run takes the goal and, when you have made one, a plan (steps with id, task, after, needs, check), which is shown on the card. Never start a run the person did not ask for. Never write a strategy script the person did not ask for; a script is shown in full on the card.";
