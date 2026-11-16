// SPDX-License-Identifier: Apache-2.0
/// Prints `NAME VALUE` for each listed variant. The match is exhaustive, so an enum
/// that gains a variant the list lacks fails to compile.
macro_rules! enum_table {
    ($($ty:ident { $($variant:ident),+ $(,)? })+) => {$(
        for value in [$($ty::$variant),+] {
            let name = match value { $($ty::$variant => stringify!($variant)),+ };
            println!("{name} {}", value as i64);
        }
    )+};
}

macro_rules! flag_table {
    ($($ty:ident { $($flag:ident),+ $(,)? })+) => {$(
        $(println!("{} {}", stringify!($flag), $ty::$flag.bits());)+
    )+};
}
