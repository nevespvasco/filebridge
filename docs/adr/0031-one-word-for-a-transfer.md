# ADR 0031 — One word for a transfer: Transfer / Envio

- Status: Accepted
- Date: 2026-10-24

## Context
The design README (Q6) decided that the UI calls a Job a **Transfer** (pt: *Transferência*). The
Portuguese sidebar, though, had always said **Envios**, and the strings drifted: "Nova
transferência" next to "Envios", "Transferências recentes" on Home, "Cancelar envio" in the
detail, and "tarefa" in one sentence. English mixed "transfer", "send" and "job" as nouns. The UX
audit of 2026-10-23 (P2) found users can't tell whether a transfer and a send are different
things.

## Decision
One noun per language, everywhere in the UI:

- English: **Transfer** ("New transfer", "Transfers", "Cancel transfer", "Transfer finished").
  "Send" stays a verb ("Send", "Sending", "Send again").
- Portuguese: **Envio** ("Novo envio", "Envios", "Envios recentes", "Cancelar envio"). "Enviar"
  stays the verb. *Transferência* and *tarefa* are no longer used (the Android folder
  "Transferências" is a proper name and stays).

*Envio* rather than *Transferência*: it was already the sidebar's word, it is shorter on the
phone's tab bar, and it matches the button that starts it ("Enviar").

## Consequences
- GLOSSARY and the design README (Q6) say so; code and docs keep saying **Job**.
- New strings use these words; a reviewer rejects a string that brings back another noun.
