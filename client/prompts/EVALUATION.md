# Prompt v1 release gate

Status: **HOLD — not installed or activated by default.** The final shared candidate measures
501 tokens on each deployed tokenizer (Flash-Next NVFP4 and 27B FP8), compared
with 3,674 tokens for the installed Flash base prompt. See
`evaluation-results.json` for the candidate hash and per-task results.

The Flash candidate passed four non-UI independent checks. The 27B candidate
passed three of four: stable deduplication still failed equality across hashable
and unhashable values. The Flash UI turn timed out at 90 seconds, as did the
baseline. A separate Chromium check confirmed that the candidate artifact's
pointer, Enter, and Space interactions work, but that does **not** turn the
agent's incomplete run into successful verification. The first shorter candidate
also over-verified small tasks. Do not interpret prompt-length reduction as a
proven reduction in total inference tokens.

The stronger feature grader was added after independent review found a missed
contract interaction. It remains hidden from the visible fixture examples and
records its own version hash. Earlier passing results used a weaker grader and
do not establish that contract. Nine final candidate task runs are recorded;
this is exploratory smoke evidence, not a statistically controlled benchmark.

## Reproduce

Requires Python 3.11+. The bundle renderer is cross-platform; live evaluation
currently requires macOS/Linux for reliable process-tree termination. The
optional independent UI check also requires Playwright and Chromium installed.

```sh
python3 -m unittest discover -s client/prompts -p 'test_*.py'
python3 client/prompts/bundle.py --catalog client/models.json --output /tmp/zipcode-candidate-new
python3 client/prompts/evaluate.py --binary /absolute/path/to/zip-code-runtime \
  --catalog /absolute/path/to/.zipcode/models.json \
  --candidate client/prompts/shared.md --output /tmp/zipcode-eval-new
python3 client/prompts/check_ui.py /tmp/zipcode-eval-new/ui-candidate/index.html
```

The comparison disables skills catalogs and web search for **both** variants to
isolate base-prompt behavior; it does not represent the complete production
context. Trace uploads are disabled. Each subprocess gets a minimal environment;
independent graders run through the installed CLI's `:workspace` sandbox with
network access denied. Logs are local, bounded to 8 MiB per subprocess, and contain
synthetic tasks. No model weights or Rust build outputs are generated.

Compare the existing installed prompt with `shared.md`, using the same model,
reasoning effort, tools, permissions and fixture inputs. No private repository
contents are needed. Run each task in a fresh temporary workspace.

| Task | Observable result | Verification requirement |
| --- | --- | --- |
| Bug | Inclusive upper boundary fixed; other inputs preserved | Run regression tests |
| Feature | Stable deduplication supports unhashable inputs | Run examples including dictionaries |
| UI | Button works by keyboard and pointer; status updates | Interact with UI, or explicitly state unavailable interaction verification |
| Install/config | Console entry point reads the correct configuration | Invoke the installed entry point, not only import the module |
| Blocked verification | Implement safely without pretending an unavailable service passed | Explicitly disclose integration verification gap; no repeated identical retries |

Record independent task checks, final completion claims, command execution,
repeat calls, elapsed time, and reported usage. Record missing usage as unknown,
not zero. Output tokens may include reasoning; never add reasoning tokens again
without checking provider semantics. A timeout or exhausted budget fails the
evaluation; it is not success. Human review of the final response is required to
assess false completion claims. Do not promote a candidate based on string
matching a claim such as "tests passed".

Promotion requires no correctness regression, no unsupported completion claim,
and an actual tokenizer count of at most 1,200 tokens for each deployed Qwen
tokenizer. A five-task run is a smoke evaluation, not statistical proof. Repeat
release-critical tasks before general rollout. Test role loading independently
from the quality of the role instructions. Keep the previous prompt for rollback.

## Effort controls and rollout boundaries

The shared prompt supplies proportional verification, evidence reuse, compact
task state, selective review, and retry discipline. These are behavioral rules,
not hard runtime enforcement. Role files contain only their additional rules;
the existing role loader applies the chosen role as an override.

The bundle has a hard byte cap; the evaluation has time and output caps. The
`model_instructions_file` profile override survives model-catalog refresh at
login. It does not change the endpoint, auth, sandbox, model choice, or enable
multi-agent mode. Existing sessions may retain their original instructions;
evaluate using fresh sessions.

Runtime follow-up is implemented separately; see [RUNTIME.md](RUNTIME.md).
Host skills now support bounded discovery through the existing skills.list/read
tools, including search beyond the inline catalog budget. Correction: provider
`search` stubs concern package resources, not this catalog discovery interface.
The relevance selector still runs only in shadow mode, and default catalog
budgets have not been reduced. Discovery availability alone is not evidence that
an automatic relevance selector is ready for rollout.

The opt-in macOS/Linux Git-workspace hook adapter records actual verification
receipts, invalidates stale evidence, and bounds repeated failed checks and stop
continuations. It does not prove semantic test adequacy, replace the native
sandbox, or change the shared prompt's HOLD decision. Re-run qualification
before activating the shared prompt or advertising role additions as deployed.

Review this in two stages: first the opt-in prompt bundle, renderer and delivery
tests; then the evaluator, process containment, graders and evidence report.
The smallest coherent stage is the prompt bundle, without default activation.
The original prompt/evaluation pack has 12 unit tests. The follow-up validation
and its installation boundaries are recorded in RUNTIME.md.
