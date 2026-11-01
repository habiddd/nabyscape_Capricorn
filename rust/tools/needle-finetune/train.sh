#!/bin/bash
# usage: train.sh <dataset dir in WSL> <epochs> <name>
cd ~/needle-tune
export NEEDLE_TELEMETRY=0 DO_NOT_TRACK=1 HF_HUB_DISABLE_TELEMETRY=1 XLA_PYTHON_CLIENT_PREALLOCATE=false
cp "$1/train.jsonl" train.jsonl
date
venv/bin/needle finetune train.jsonl --epochs "$2" --batch-size 2 --max-len 1024 --lora-rank 8 --out "$3.safetensors" 2>&1 | grep -E 'epoch|accuracy|data |adapter|note|Error|error' | cut -c1-200
date
JAX_PLATFORMS=cpu venv/bin/needle build checkpoints/needle3.safetensors --lora "$3.safetensors" --out "$3.cact" 2>&1 | tail -4 | cut -c1-200
ls -la "$3.cact" && cp "$3.cact" "$1/$3.cact"
sha256sum "$3.cact"
