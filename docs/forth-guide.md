[English Version](forth-guide-en.md)

# MerlionOS Forth 编程指南

## 简介

MerlionOS 内置了一个 Forth 解释器——一种基于栈的编程语言。Forth 于 1970 年由 Charles Moore 发明，以其极简设计和直接操作硬件的能力闻名。在 MerlionOS 中，Forth 让你可以在操作系统内进行即时计算和编程。

## 启动

```
merlion> forth
MerlionOS Forth v1.0
Type 'words' for built-ins, 'exit' to quit.
forth>
```

## 基础：栈操作

Forth 使用逆波兰表示法（RPN）——先写操作数，再写操作符：

```forth
forth> 3 4 +
 ok
forth> .
7 ok
```

`.` 弹出并打印栈顶值。`.s` 显示整个栈：

```forth
forth> 10 20 30
 ok
forth> .s
<3> 10 20 30
```

### 栈操作词

| 词 | 效果 | 示例 |
|----|------|------|
| `dup` | 复制栈顶 | `5 dup .s` → `<2> 5 5` |
| `drop` | 丢弃栈顶 | `1 2 drop .s` → `<1> 1` |
| `swap` | 交换顶两个 | `1 2 swap .s` → `<2> 2 1` |
| `over` | 复制第二个到顶 | `1 2 over .s` → `<3> 1 2 1` |
| `rot` | 旋转前三个 | `1 2 3 rot .s` → `<3> 2 3 1` |
| `depth` | 栈深度 | `1 2 depth .` → `2` |

## 算术

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

| 词 | 操作 |
|----|------|
| `+` `-` `*` `/` | 加减乘除 |
| `mod` | 取余 |
| `negate` | 取负 |
| `abs` | 绝对值 |
| `max` `min` | 最大/最小值 |

## 比较

比较返回 `-1`（真）或 `0`（假）：

```forth
forth> 5 3 > .
-1  ok

forth> 5 5 = .
-1  ok

forth> 0 0= .
-1  ok
```

| 词 | 含义 |
|----|------|
| `=` | 相等 |
| `<` `>` | 小于/大于 |
| `0=` | 等于零 |

## 逻辑

```forth
forth> -1 -1 and .
-1  ok

forth> 0 -1 or .
-1  ok

forth> -1 not .
0  ok
```

## 自定义词（函数）

用 `: name body ;` 定义新词：

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

词可以调用其他词：

```forth
forth> : hyp-sq square swap square + ;
 ok

forth> 3 4 hyp-sq .
25  ok
```

## 输出

```forth
forth> 65 emit
A ok

forth> cr
                    (换行)

forth> 72 emit 101 emit 108 emit 108 emit 111 emit cr
Hello
```

| 词 | 效果 |
|----|------|
| `.` | 打印并弹出栈顶数字 |
| `.s` | 显示整个栈（不弹出） |
| `cr` | 换行 |
| `emit` | 打印 ASCII 字符 |

## 变量

```forth
forth> variable x
 ok

forth> 42 0 !
 ok

forth> 0 @ .
42  ok
```

> 注：当前变量通过索引访问（第一个变量索引 0，第二个 1...）

## 返回栈

```forth
forth> 5 >r      ( 将 5 移到返回栈 )
forth> r> .       ( 从返回栈取回 )
5  ok
```

## 查看所有词

```forth
forth> words
Built-in: + - * / mod dup drop swap over rot . .s cr emit = < > depth
User: square cube hyp-sq
```

## 退出

```forth
forth> exit
Forth exited.
merlion>
```

## 经典示例

### 阶乘

```forth
: fact dup 1 > if dup 1 - fact * then ;
10 fact .
3628800
```

> 注：`if...then` 目前未实现完整的控制流。上面的递归阶乘需要未来版本支持。

### 斐波那契

```forth
: fib dup 2 < if drop 1 else dup 1 - fib swap 2 - fib + then ;
```

### 温度转换

```forth
: c-to-f 9 * 5 / 32 + ;
100 c-to-f .
212
```

### 打印星号行

```forth
: stars 0 do 42 emit loop cr ;
5 stars
*****
```

> 注：`do...loop` 需要未来版本支持。

## 为什么在 OS 中嵌入 Forth？

1. **历史传统** — Open Firmware (Sun/Apple 的 BIOS) 用的就是 Forth
2. **极简实现** — 整个解释器只有 290 行 Rust
3. **即时编程** — 不需要编译器，直接在命令行定义函数
4. **硬件亲和** — Forth 天然适合直接操作内存和 I/O 端口
5. **可扩展性** — 用户定义的词和内置词没有区别

## 未来扩展

- `if...else...then` 条件分支
- `do...loop` 循环
- `begin...until` / `begin...while...repeat` 循环
- 字符串操作
- 通过 Forth 直接读写 I/O 端口和内存
- 从 VFS 文件加载 Forth 脚本
