---
name: Bug report
about: Something behaved incorrectly
labels: bug
---

**Version and platform**

- `ripsed --version`:
- OS:
- Install method (cargo install / release binary / source):

**Minimal reproduction**

```bash
# Smallest input + command that shows the problem. For file-mode bugs,
# include the file content (watch for CRLF/encoding — `xxd | head`
# output helps for byte-level issues).
```

**Expected behavior**

**Actual behavior**

Include the exit code if relevant (`echo $?`) — ripsed uses
0 = changed, 1 = no matches, 2 = error.
