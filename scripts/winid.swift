import CoreGraphics
let pid = Int32(CommandLine.arguments[1])!
let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
for w in list {
  if (w[kCGWindowOwnerPID as String] as? Int32) == pid, (w[kCGWindowLayer as String] as? Int) == 0,
     let n = w[kCGWindowNumber as String] { print(n); break }
}
