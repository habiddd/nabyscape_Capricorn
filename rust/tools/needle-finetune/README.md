# Needle fine-tune for quick commands

Local data only; nothing goes to OpenRouter or Cactus. Tested in WSL2 Ubuntu on an RTX 3050 (4 GB),
about 27 minutes. Batch 2 and rank 8 are what fit in 4 GB.

    python3 -m venv ~/needle-tune/venv
    ~/needle-tune/venv/bin/pip install "cactus-needle[train,gpu]==3.2.8"
    python make_data.py ../../crates/naby-core/src/assistant/router.rs <dir>
    bash train.sh <dir in WSL> 3 tuned
    python exam.py needle.exe <dir>/tuned.cact <dir>

Then replace `rust/models/needle3-es.cact` with `tuned.cact` and set `TUNED_MODEL_SHA256` in
`naby-win/src/voice/whistle.rs`; `build-release.ps1` refuses a mismatch. Keep 20 layers: 12 and 8 layers lost most
of what was learned (false actions 8 and 24 of 30, against 4). The exam runs without the app's
word checks; add phrases there, never to the training data.
