[中文版](forth-guide.md)

# MerlionOS Forth Programming Guide

## Introduction

MerlionOS includes a built-in Forth interpreter — a stack-based programming language. Forth was invented by Charles Moore in 1970 and is renowned for its minimalist design and ability to directly manipulate hardware. In MerlionOS, Forth lets you perform instant computation and programming right inside the operating system.

## Getting Started

```
merlion> forth
MerlionOS Forth v1.0
Type 'words' for built-ins, 'exit' to quit.
forth>
```

## Basics: Stack Operations

Forth uses Reverse Polish Notation (RPN) — operands come first, then the operator:

```forth
forth> 3 4 +
 ok
forth> .
7 ok
```

`.` pops and prints the top-of-stack value. `.s` displays the entire stack:

```forth
forth> 10 20 30
 ok
forth> .s
<3> 10 20 30
```

### Stack Manipulation Words

| Word | Effect | Example |
|------|--------|---------|
| `dup` | Duplicate top of stack | `5 dup .s` → `<2> 5 5` |
| `drop` | Discard top of stack | `1 2 drop .s` → `<1> 1` |
| `swap` | Swap top two elements | `1 2 swap .s` → `<2> 2 1` |
| `over` | Copy second element to top | `1 2 over .s` → `<3> 1 2 1` |
| `rot` | Rotate top three elements | `1 2 3 rot .s` → `<3> 2 3 1` |
| `depth` | Stack depth | `1 2 depth .` → `2` |

## Arithmetic

```forth
forth> 10 3 + .
13  ok

forth> 100 7 / .
14  ok

forth> 100 7 mod .
2  ok

forth> -5 abs .
5  ok

forth> 3 7 max .
7  ok
```

| Word | Operation |
|------|-----------|
| `+` `-` `*` `/` | Addition, subtraction, multiplication, division |
| `mod` | Modulo |
| `negate` | Negate |
| `abs` | Absolute value |
| `max` `min` | Maximum / Minimum |

## Comparison

Comparisons return `-1` (true) or `0` (false):

```forth
forth> 5 3 > .
-1  ok

forth> 5 5 = .
-1  ok

forth> 0 0= .
-1  ok
```

| Word | Meaning |
|------|---------|
| `=` | Equal |
| `<` `>` | Less than / Greater than |
| `0=` | Equal to zero |

## Logic

```forth
forth> -1 -1 and .
-1  ok

forth> 0 -1 or .
-1  ok

forth> -1 not .
0  ok
```

## Defining Custom Words (Functions)

Use `: name body ;` to define a new word:

```forth
forth> : square dup * ;
 ok

forth> 7 square .
49  ok

forth> : cube dup dup * * ;
 ok

forth> 3 cube .
27  ok
```

Words can call other words:

```forth
forth> : hyp-sq square swap square + ;
 ok

forth> 3 4 hyp-sq .
25  ok
```

## Output

```forth
forth> 65 emit
A ok

forth> cr
                    (newline)

forth> 72 emit 101 emit 108 emit 108 emit 111 emit cr
Hello
```

| Word | Effect |
|------|--------|
| `.` | Print and pop top-of-stack number |
| `.s` | Display entire stack (non-destructive) |
| `cr` | Newline |
| `emit` | Print ASCII character |

## Variables

```forth
forth> variable x
 ok

forth> 42 0 !
 ok

forth> 0 @ .
42  ok
```

> Note: Variables are currently accessed by index (first variable is index 0, second is 1, etc.)

## Return Stack

```forth
forth> 5 >r      ( move 5 to return stack )
forth> r> .       ( retrieve from return stack )
5  ok
```

## Listing All Words

```forth
forth> words
Built-in: + - * / mod dup drop swap over rot . .s cr emit = < > depth
User: square cube hyp-sq
```

## Exiting

```forth
forth> exit
Forth exited.
merlion>
```

## Classic Examples

### Factorial

```forth
: fact dup 1 > if dup 1 - fact * then ;
10 fact .
3628800
```

> Note: `if...then` does not yet have full control flow support. The recursive factorial above requires a future version.

### Fibonacci

```forth
: fib dup 2 < if drop 1 else dup 1 - fib swap 2 - fib + then ;
```

### Temperature Conversion

```forth
: c-to-f 9 * 5 / 32 + ;
100 c-to-f .
212
```

### Print a Row of Stars

```forth
: stars 0 do 42 emit loop cr ;
5 stars
*****
```

> Note: `do...loop` requires a future version.

## Why Embed Forth in an OS?

1. **Historical tradition** — Open Firmware (Sun/Apple BIOS) used Forth
2. **Minimal implementation** — The entire interpreter is only 290 lines of Rust
3. **Instant programming** — No compiler needed; define functions directly at the command line
4. **Hardware affinity** — Forth is naturally suited for direct memory and I/O port manipulation
5. **Extensibility** — User-defined words are indistinguishable from built-in words

## Future Extensions

- `if...else...then` conditional branching
- `do...loop` loops
- `begin...until` / `begin...while...repeat` loops
- String operations
- Direct I/O port and memory read/write via Forth
- Loading Forth scripts from VFS files
