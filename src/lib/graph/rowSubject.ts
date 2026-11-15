import type { CommitRow, RepoSnapshot } from "../types";

/**
 * The subject a row shows. The uncommitted row's is the snapshot's `uncommittedSubject`: a
 * change count that changes moves no row, so it comes without a new generation, and rows
 * already loaded keep the subject they were read with.
 */
export function rowSubject(row: CommitRow, snapshot: Pick<RepoSnapshot, "uncommittedSubject">): string {
  return row.kind === "uncommitted" ? snapshot.uncommittedSubject : row.subject;
}
