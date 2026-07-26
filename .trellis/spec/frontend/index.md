# Frontend Development Guidelines

> Best practices for frontend development in this project.

---

## Overview

This directory contains guidelines for frontend development. Fill in each file with your project's specific conventions.

---

## Guidelines Index

| Guide | Description | Status |
|-------|-------------|--------|
| [Directory Structure](./directory-structure.md) | Module organization and file layout | To fill |
| [Component Guidelines](./component-guidelines.md) | Component patterns, props, composition, and virtualized-row motion | Active |
| [Hook Guidelines](./hook-guidelines.md) | 抽 hook 的共现分析判据、ref 间接层、聚合视图 getter、稳定身份契约 | Active |
| [State Management](./state-management.md) | Local state, global state, server state | To fill |
| [Quality Guidelines](./quality-guidelines.md) | 无 e2e 下的三层验证、变异验证硬要求、断言陷阱、重构审阅清单 | Active |
| [Type Safety](./type-safety.md) | Type patterns, validation | To fill |
| [Conversation Export](./conversation-export.md) | Markdown export command, privacy boundary, and validation contract | Active |
| [Replace Translation](./replace-translation.md) | Offline pack, MI-GAN mask, stable-ID event, and natural-size Canvas contract | Active |
| [macOS Overlay Lifecycle](./macos-overlay-lifecycle.md) | Nonactivating Lens/translator focus, foreground, and teardown contract | Active |
| [Product Identity and Packaging](./product-identity-and-packaging.md) | Display-name rename boundary, stable identifiers, and release artifact contract | Active |
| [Model Request Parameters](./model-request-parameters.md) | Optional model metadata, override tombstones, validation, and request serialization | Active |
| [Built-in Web Search](./builtin-web-search.md) | Session web-search mode, hosted-search injection/citation wire contracts, realtime card, effort mapping | Active |
| [External Agent Integration](./external-agent-integration.md) | Project-scoped CLI discovery, cache keys, and process shutdown contracts | Active |
| [Resource Lifecycle](./resource-lifecycle.md) | Child/Worker/server shutdown, idle cleanup, weak registries, and bounded runtime caches | Active |

---

## How to Fill These Guidelines

For each guideline file:

1. Document your project's **actual conventions** (not ideals)
2. Include **code examples** from your codebase
3. List **forbidden patterns** and why
4. Add **common mistakes** your team has made

The goal is to help AI assistants and new team members understand how YOUR project works.

---

**Language**: All documentation should be written in **English**.
