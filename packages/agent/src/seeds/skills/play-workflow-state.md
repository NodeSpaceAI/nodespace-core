# Play Workflow State Guidance

This skill answers "why hasn't this Play rule fired?" or "what's still missing before it will?" for a node governed by NodeSpace's Play automation system (trigger → conditions → actions).

CALL get_workflow_state WITH THE NODE'S ID: it evaluates every active Play rule whose trigger could apply to that node's type against the node's current state, and reports each rule's conditions as one of: satisfied, not yet met (a real, schema-declared relationship or field that just doesn't have a value yet — normal, the Play stays active), or unresolvable (the condition references something that isn't a declared field or relationship on the node's schema at all — almost certainly a typo in how the Play was authored, and will never resolve no matter what the graph looks like).

SCOPE: this reports live condition state computed right now, on this device — it is not an execution history. Whether a rule has already fired is not tracked anywhere in the system today, so never tell the user a rule "already ran" or "hasn't run yet" based on this tool; only report what conditions currently hold.

UNRESOLVABLE MEANS LIKELY MISAUTHORED: if a condition comes back unresolvable and the response's degraded_reasons list is empty, say so plainly and name the specific unresolvable path — don't describe it as "not yet met," which implies waiting will fix it. Waiting will not fix a typo.

NON-EMPTY degraded_reasons MEANS INCOMPLETE: a lookup failed while the response was built, so it may be missing rules and may report a real field or relationship as unresolvable. Call get_workflow_state once more for the same node. If degraded_reasons is still non-empty, report what came back but tell the user the result may be incomplete, and never call a condition a typo or misauthored on the strength of it.

FIND THE NODE FIRST: if you don't already have the node's id, call search_semantic or search_nodes first, then call get_workflow_state with the resolved id.
