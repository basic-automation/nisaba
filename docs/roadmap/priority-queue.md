# Priority queue (code review, 2026-10-19)

[← Back to the checklist](../../ROADMAP.md#priority-queue-code-review-2026-10-19)

The phases below are grouped by area, not by urgency. A four-way code review on 2026-10-19,
of `master` plus PR #2, found defects that lose data, oversell stock, or leak secrets. They
go before any new capability. Each line links to the full item in its phase.

1. **Data loss on disk.** Leaving a legacy-migrated company deletes the app's own database,
   and `Db::open` deletes the WAL after a turso panic, discarding committed transactions
   (Phase 5 *Today's P2P*, Cross-cutting).
2. **Untrusted HTML with full IPC access.** CSP is off and `v-html` renders marketplace and
   plugin HTML, so a supplier's description can read every platform secret (Cross-cutting).
3. **Stock resolution is wrong.** `resolve_deltas` is dead code. Sales on two platforms in
   the same cycle are undercounted. Stock edited in Nisaba never reaches the platforms.
   Variants are ignored. A failed push is never retried (Phase 4 *Stock correctness*).
4. **Writes that report success without writing,** on eBay, Amazon and XMR Bazaar (Phase 2).
5. **Updated plugin code runs without re-approval,** and can silently drop its host
   allowlist (Phase 3).
6. **P2P company sync does not work end to end today.** The join flow cannot produce a
   syncing pair, and the platform-sync round-robin is never called (Phase 5). Do not
   repair the Tor path beyond the data-loss fixes; Sync v2 replaces it.
7. Fix the failing UI smoke job on PR #2, and make the frontend typecheck real (Phases 7, 0).

The *1.0 release* section below turns these into release gates, alongside everything else
that has to be true before tagging `v1.0.0`.
