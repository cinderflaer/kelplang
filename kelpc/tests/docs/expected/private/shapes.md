# Module `shapes`

Shapes: a small module that shows every kind of documented item.

See [`Shape`](#shape) for the trait and [`Circle`](#circle) for a type that implements it.

## Imports

- `impl "std/fs" caps(fs = ["/tmp/**"])`
- `impl "std/str"`

## Types

### `Circle`

```kelp
pub datac Circle(val r: f64, var tag: i32)
```

A circle.

- the radius is in `r`
- [`Kind`](#kind) says what sort of shape it is

**Fields**

- `val r: f64`: The radius.
- `var tag: i32`: A free tag.

#### `Circle.diameter`

```kelp
fun Circle.diameter() => f64
```

The diameter.

**Trait impls**

- `impl` [`Shape`](#shape)` for Circle`

### `Kind`

```kelp
pub enum Kind
```

Kinds of shape.

**Variants**

- `Round(val r: f64)`: A round one.
- `Flat`: A flat one.

### `Private`

```kelp
datac Private(val a: i32)
```

**Fields**

- `val a: i32`

## Traits

### `Shape`

```kelp
pub trait Shape
```

Something with an area.

**Required and provided items**

- `fun Self.area() => f64`: The area.
- `type Item`

## Functions

### `hidden`

```kelp
fun hidden()
```

### `load`

```kelp
pub fun load(path: &str) => String
```

*requires capability `fs`*

Reads a file named by `path`.

### `make`

```kelp
pub unsafe fun make(r: f64) => Circle
```

*unsafe*

Makes a circle from a radius.

## Constants

### `PI2`

```kelp
pub const PI2: f64 = 6.28
```

Pi, roughly.

### `SECRET`

```kelp
const SECRET: i32 = 7
```

## Statics

### `COUNT`

```kelp
pub static mut COUNT: i32 = 0
```

A counter shared by every shape.

## Passdowns

### `scale`

```kelp
pd val scale = 2
```

A passdown nobody outside can see.
