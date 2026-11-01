"""Scores a .cact on the held-out exam with needle.exe, without the app's word checks."""
import json, subprocess, sys, time, os

exe, model, out = sys.argv[1], sys.argv[2], sys.argv[3]
exam = json.load(open(f"{out}/exam.json", encoding="utf-8"))
env = dict(os.environ, NEEDLE_TELEMETRY="0", DO_NOT_TRACK="1")
right = missed = wrong = refused_ok = false_act = 0
total_t = 0.0
for prompt, want in exam:
    t = time.time()
    r = subprocess.run([exe, "--model", model, "--tools", f"{out}/tools.json", "--prompt", prompt], capture_output=True, text=True, encoding="utf-8", errors="replace", env=env)
    total_t += time.time() - t
    lines = [l for l in r.stdout.splitlines() if l.startswith("{")]
    if not lines:
        print("NO OUTPUT", prompt, (r.stdout + r.stderr)[:300])
        continue
    d = json.loads(lines[-1])
    calls = d.get("function_calls") or []
    got = None
    if calls:
        got = (calls[0]["name"] + " " + " ".join(str(v) for v in calls[0]["arguments"].values())).strip()
    if want:
        if got == want:
            right += 1
        elif got is None:
            missed += 1
            print("MISSED", prompt, "want", want, "held", (d.get("suppressed_calls") or [None])[0])
        else:
            wrong += 1
            print("WRONG ", prompt, "want", want, "got", got, d.get("confidence"))
    else:
        if got is None:
            refused_ok += 1
        else:
            false_act += 1
            print("FALSE ", prompt, "got", got, d.get("confidence"))
pos = sum(1 for _, w in exam if w)
print(f"{os.path.basename(model)}: commands right {right}/{pos}, missed {missed}, wrong action {wrong}; conversation left alone {refused_ok}/{len(exam)-pos}, false actions {false_act}; avg {total_t/len(exam):.2f} s; peak_ram {d.get('peak_ram_mb')} confidence {d.get('confidence')}")
