// The menu of a tag label (spec §5.2, "Tag").

import type { RefLabel, RepoSnapshot } from "../types";
import { item, noRemoteReason, SEPARATOR } from "./common";
import type { MenuItem } from "./types";

export function tagMenu(_label: RefLabel, snap: RepoSnapshot): MenuItem[] {
  return [
    item("push", "Push…", { disabledReason: noRemoteReason(snap) }),
    item("delete", "Delete…", { danger: true }),
    SEPARATOR,
    item("copy.name", "Copy name"),
  ];
}
