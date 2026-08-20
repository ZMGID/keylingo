---
id: docx
name: docx
description: Read, summarize, revise, and analyze Word DOC/DOCX attachments saved by Kivio Chat.
recommended-tools:
  - read
  - bash
---

# DOCX Skill

Use this skill when the user attaches or references a Word document (`.doc` or `.docx`) and asks to read, summarize, revise, extract, compare, translate, or answer questions about it.

## Inputs

Kivio stores each uploaded document as a safe local copy and includes its absolute path in the user message under `Kivio 安全副本路径`. Use that host path with `bash`. You can also process any local Word document discovered via `glob` / `read` (directory listing).

`read` does not parse binary Word files.

## Workflow

1. Identify the safe copy path from the attachment note.
2. If the host has a Word tool (`python` with python-docx, OfficeCLI, etc.), extract text with `bash`. For a multi-line script, `write` it first, then run it.
3. A `.docx` is a zip of XML; if Python is available you can unzip `word/document.xml` and collect `w:t` nodes. Legacy `.doc` usually needs conversion to `.docx`.
4. If no extraction tool is installed, say so. Do not invent content that was not extracted.

## Output

- For summaries: group by headings when possible.
- For edits: state what changed and provide replacement text or a concise revision plan.
- For extraction: keep document order and mark unclear formatting honestly.
