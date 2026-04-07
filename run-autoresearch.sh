#!/bin/bash
set -e
cd ~/lichen
exec claude --permission-mode bypassPermissions --print "$(cat AUTORESEARCH_PROMPT.md)" >> output/hypotheses/claude-code.log 2>&1
