---
id: xlsx
name: xlsx
description: Read, summarize, calculate from, and analyze Excel/CSV/TSV spreadsheet attachments saved by Kivio Chat.
recommended-tools:
  - read
  - bash
---

# XLSX Skill

Use this skill when the user attaches or references a spreadsheet (`.xls`, `.xlsx`, `.xlsm`, `.csv`, or `.tsv`) and asks to inspect, summarize, calculate, clean, compare, chart, or answer questions from it.

## Inputs

Kivio stores each uploaded spreadsheet as a safe local copy and includes its absolute path in the user message under `Kivio 安全副本路径`. Use that host path with `read` or `bash`. You can also process any local spreadsheet discovered via `glob` / `read` (directory listing).

## Workflow

1. Identify the safe copy path from the attachment note.
2. For `.csv` / `.tsv`, use `read` for a small text preview.
3. For `.xlsx` / `.xls` / `.xlsm`, `read` does not parse the workbook. If the host has Excel tooling (`python` with pandas/openpyxl, OfficeCLI, etc.), inspect sheets with `bash`. For a multi-line script, `write` it first, then run it.
4. If no spreadsheet tool is installed, say so. Do not invent numbers.
5. Inspect sheet names, columns, row counts, missing values, and representative rows before answering. Run calculations explicitly.

## Output

- For analysis: include calculation assumptions and key columns used.
- For summaries: mention sheet names and row/column counts.
- For charts: write generated files into the current workbench, then call `present_artifacts` only when the user should see them.
