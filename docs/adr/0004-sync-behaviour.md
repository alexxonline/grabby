# 4. Text sync behaviour

Status: accepted. The wire contract is in `SYNC.md`.

## Decisions

- Errors are reduced to a kind name. Status and logs never contain provider responses, URLs or credentials.
- Automatic and manual sync refuse to run while a recording is captured, being saved or processed, so they cannot overwrite it.
- When independently imported recordings share a notes ID, the incoming notes get a new ID so both texts are kept.
- The mock receiver serves one configured local test account and keeps records in insertion order.
