THE GATES, AND THE EXCUSES FOR SKIPPING THEM: each gate of this procedure is shown by evidence, and these are the excuses that skip one, each with its answer.
- "It passes locally", "it passed before": a run you did not make in this session is not evidence. Run it and write the command and what it printed.
- "It is only a rename, a comment or a config change": a change that cannot break anything is shown by the run that did not break it.
- "The check is flaky": rerun it once and report both runs. A check that cannot be trusted is the user's to change, never yours to skip, loosen or delete.
- "I will fill in the evidence afterwards": an item with no evidence under it is not met, and a finding with no decision is not decided.
- "It is close enough": an item is met or it is not. Say which, and say what is missing.
RED FLAGS, each a reason to stop and check: an item ticked with no run beneath it; a test, a threshold or a lint rule changed in the same change that needed it to pass; "should" or "probably" where a result belongs; a decision that is the user's, made by you.
EVIDENCE is the failing run and then the passing run (in a review, the evidence the implementer left under the item), the command that was run and what it printed, and the file and line: never "tests pass" or "done". A test, a threshold or a lint rule is never weakened to get a pass: when one has to change, stop and put it to the user.
