# Regenerate:
#   brew install vhs
#   ./docs/generate-blame-fixture.sh
#   vhs docs/blame-demo.tape
#
# Drives a real fixture repo (a genuine agentrec daemon session was
# recorded into it) — every line below is real CLI output, not staged text.

Output docs/blame-demo.gif

Require agentrec

Set Shell "bash"
Set FontSize 22
Set FontFamily "Menlo"
Set Width 1040
Set Height 480
Set Padding 20
Set Theme "Dracula"
Set TypingSpeed 45ms

Hide
Type `cd docs/fixtures/demo-repo && export PATH="$PWD/../../../target/release:$PATH" && export PS1='$ ' && clear` Enter
Sleep 500ms
Show

Type "agentrec blame src/auth.ts:42"
Sleep 700ms
Enter
Sleep 2s

Type `echo '// TODO: double-check limiter window under load' >> src/auth.ts`
Sleep 600ms
Enter
Sleep 800ms

Type "agentrec blame src/auth.ts"
Sleep 700ms
Enter
Sleep 3s
