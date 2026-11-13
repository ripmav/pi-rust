/**
 * WP-704 e2e fixture: records the compaction events in the session file via
 * pi.appendEntry (the core persists them as `custom` entries), which the
 * e2e tests assert on:
 * - session_before_compact -> customType "wp704-before-compact"
 *   data { reason, willRetry, branchEntries, cancel }
 * - session_compact        -> customType "wp704-compacted"
 *   data { summary, firstKeptEntryId, tokensBefore, fromExtension, reason, willRetry }
 *
 * With WP704_CANCEL=1 in the sidecar env, the before-compact handler answers
 * {cancel: true} instead of proceeding (cancel path). Test-only fixture, not
 * an unmodified upstream extension.
 */
export default function (pi) {
	const cancel = process.env.WP704_CANCEL === "1";
	pi.on("session_before_compact", (event) => {
		pi.appendEntry("wp704-before-compact", {
			reason: event.preparation?.reason,
			willRetry: event.preparation?.willRetry,
			branchEntries: (event.branchEntries ?? []).length,
			cancel,
		});
		return cancel ? { cancel: true } : undefined;
	});
	pi.on("session_compact", (event) => {
		pi.appendEntry("wp704-compacted", {
			summary: event.compactionEntry?.summary,
			firstKeptEntryId: event.compactionEntry?.firstKeptEntryId,
			tokensBefore: event.compactionEntry?.tokensBefore,
			fromExtension: event.fromExtension,
			reason: event.reason,
			willRetry: event.willRetry,
		});
	});
}
