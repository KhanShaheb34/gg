# Third-party notices

gg's own code is under the [MIT licence](LICENSE). Parts of it come from other projects, as
said here.

## Orca

The default prompts of Generate with AI (a commit message, and a pull request's title and
description), the way the diff is cut to fit them, and the way the tool's answer is read
(`crates/gg-core/src/ai/prompt.rs`) are taken from Orca
(https://github.com/stablyai/orca: `src/shared/source-control/ai/generation/`), adapted for gg
as that file says.

The title a new pull request is given from its branch's name (`branch_title` in
`crates/gg-core/src/gh.rs`) is taken from Orca (https://github.com/stablyai/orca:
`resolveCreateReviewDraftTitle` in
`src/shared/source-control/hosted-review/hosted-review-create-seed.ts`, with
`humanizeBranchSlug` and `normalizeHostedReviewHeadRef`), adapted for gg as that function
says.

```text
MIT License

Copyright (c) 2026 Lovecast Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```
