# Malformed lines

These should not crash the parser; some should not be recognized as tasks at
all.

- [ task missing closing bracket
- [?] unknown status marker should parse as Open
- []missing space
-[ ] missing space after dash
  -- not a task
- [   ] extra whitespace inside brackets
- [ ]   extra spaces after status

A list with prose only:

- this is a list item but not a task
- another list item

A heading-only file:

## Heading

End of file with no trailing newline at end → handled separately by parser.
