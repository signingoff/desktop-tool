# Serenity — Supply Chain Chokepoint Analyst (Claude Code Skill)

> 中文说明 → [README.zh-CN.md](README.zh-CN.md)

A self-contained [Claude Code](https://claude.com/claude-code) skill that replicates the research process, voice, 7-step playbook, and risk discipline of X analyst **Serenity (@aleabitoreddit)** — an AI semiconductor / photonics / CPO / InP / Physical AI supply-chain analyst.

The edge: *trading unknown bottlenecks* — mapping obscure chokepoints in the AI hardware stack (optical interconnects / CPO, upstream materials like InP / SOI / TFLN, precision components for Physical AI / robotics) where physical reality diverges from market narratives.

## Files

| File | Job |
|------|-----|
| `SKILL.md` | Skill entry point — persona, 7-step playbook, output rules, hard constraints, modes |
| `memory.md` | The "brain" — holdings table, full playbook with quotes, risk/sizing rules, few-shot style examples |
| `rubric.md` | Fidelity scoring rubric (5 dimensions × 0–5 = /25) |

## Install

Clone into your Claude Code skills directory:

```bash
git clone https://github.com/BillyLu365/serenity-skill.git ~/.claude/skills/serenity
```

Then invoke it in Claude Code with `/serenity`, or ask for a supply-chain chokepoint thesis, to pressure-test a ticker idea, or "what would Serenity think".

## Disclaimer

This replicates a public research **process and style** — not anyone's capital, exact sizing, or private information. **Not financial advice.**
