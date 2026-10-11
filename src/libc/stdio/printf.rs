/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `printf` function family. The implementation is also used by `NSLog` etc.

use crate::abi::{DotDotDot, VaList};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::{ns_string, unichar};
use crate::libc::clocale::{setlocale, LC_CTYPE};
use crate::libc::errno::set_errno;
use crate::libc::posix_io::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use crate::libc::stdio::{fwrite, getc, ungetc, EOF, FILE};
use crate::libc::stdlib::{atof_inner_generic, str_to_int_inner_generic};
use crate::libc::wchar::wchar_t;
use crate::mem::{guest_size_of, ConstPtr, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr};
use crate::objc::{id, msg, nil};
use crate::Environment;
use std::collections::HashSet;
use std::io::Write;

#[derive(Clone, Copy, Default)]
struct FormatFlags {
    sign: bool,
    space: bool,
    alternate: bool,
    zero: bool,
    left: bool,
}

enum FormatCount {
    Literal(usize),
    Argument,
}

struct Conversion {
    flags: FormatFlags,
    width: FormatCount,
    precision: Option<FormatCount>,
    length: Option<&'static str>,
    specifier: u8,
}

impl Conversion {
    /// Parse one conversion, starting just after '%'. Argument widths and
    /// precisions are resolved separately, before reading the value itself.
    fn parse(get: impl Fn(usize) -> u8, index: &mut usize) -> Self {
        let mut flags = FormatFlags::default();
        loop {
            match get(*index) {
                b'+' => flags.sign = true,
                b' ' => flags.space = true,
                b'#' => flags.alternate = true,
                b'0' => flags.zero = true,
                b'-' => flags.left = true,
                _ => break,
            }
            *index += 1;
        }
        let count = |index: &mut usize| {
            if get(*index) == b'*' {
                *index += 1;
                FormatCount::Argument
            } else {
                let mut value = 0;
                while let c @ b'0'..=b'9' = get(*index) {
                    value = value * 10 + (c - b'0') as usize;
                    *index += 1;
                }
                FormatCount::Literal(value)
            }
        };
        let width = count(index);
        let precision = if get(*index) == b'.' {
            *index += 1;
            Some(count(index))
        } else {
            None
        };
        let length = match get(*index) {
            b'h' | b'l' => {
                let c = get(*index);
                *index += 1;
                let doubled = get(*index) == c;
                if doubled {
                    *index += 1;
                }
                Some(match (c, doubled) {
                    (b'h', false) => "h",
                    (b'h', true) => "hh",
                    (b'l', false) => "l",
                    _ => "ll",
                })
            }
            b'q' | b'j' => {
                *index += 1;
                Some("ll")
            }
            b'z' | b't' => {
                *index += 1;
                Some("l")
            }
            b'L' => {
                *index += 1;
                Some("L")
            }
            _ => None,
        };
        let specifier = get(*index);
        *index += 1;
        Self {
            flags,
            width,
            precision,
            length,
            specifier,
        }
    }

    fn resolve(&self, mut next: impl FnMut() -> i32) -> FieldFormat {
        let mut flags = self.flags;
        let width = match self.width {
            FormatCount::Literal(value) => value,
            FormatCount::Argument => {
                let width = next();
                flags.left |= width < 0;
                width.unsigned_abs() as usize
            }
        };
        let precision = match self.precision {
            Some(FormatCount::Literal(value)) => Some(value),
            Some(FormatCount::Argument) => usize::try_from(next()).ok(),
            None => None,
        };
        FieldFormat {
            flags,
            width,
            precision,
        }
    }
}

struct FieldFormat {
    flags: FormatFlags,
    width: usize,
    precision: Option<usize>,
}

impl FieldFormat {
    fn sign(&self, negative: bool) -> &'static str {
        if negative {
            "-"
        } else if self.flags.sign {
            "+"
        } else if self.flags.space {
            " "
        } else {
            ""
        }
    }

    /// Zero padding follows the sign and radix prefix. Left adjustment always
    /// wins over zero padding; integer precision also disables zero padding.
    fn number(&self, prefix: &str, digits: &str, zero: bool) -> Vec<u8> {
        let padding = self.width.saturating_sub(prefix.len() + digits.len());
        let mut result = Vec::new();
        if !self.flags.left && !zero {
            result.resize(padding, b' ');
        }
        result.extend_from_slice(prefix.as_bytes());
        if !self.flags.left && zero {
            result.resize(result.len() + padding, b'0');
        }
        result.extend_from_slice(digits.as_bytes());
        if self.flags.left {
            result.resize(result.len() + padding, b' ');
        }
        result
    }

    fn text(&self, bytes: &[u8]) -> Vec<u8> {
        let padding = self.width.saturating_sub(bytes.len());
        let mut result = Vec::new();
        if !self.flags.left {
            result.resize(padding, b' ');
        }
        result.extend_from_slice(bytes);
        if self.flags.left {
            result.resize(result.len() + padding, b' ');
        }
        result
    }

    fn integer(&self, magnitude: u64, negative: bool, specifier: u8) -> Vec<u8> {
        let mut digits = match specifier {
            b'o' => format!("{magnitude:o}"),
            b'x' | b'p' => format!("{magnitude:x}"),
            b'X' => format!("{magnitude:X}"),
            _ => magnitude.to_string(),
        };
        if self.precision == Some(0) && magnitude == 0 {
            digits.clear();
        }
        if let Some(precision) = self.precision {
            digits = format!(
                "{}{}",
                "0".repeat(precision.saturating_sub(digits.len())),
                digits
            );
        }
        let prefix = match specifier {
            b'd' | b'i' => self.sign(negative),
            b'x' if self.flags.alternate && magnitude != 0 => "0x",
            b'X' if self.flags.alternate && magnitude != 0 => "0X",
            b'o' if self.flags.alternate && !digits.starts_with('0') => "0",
            // iOS formats a null pointer as 0x0, too.
            b'p' => "0x",
            _ => "",
        };
        self.number(prefix, &digits, self.flags.zero && self.precision.is_none())
    }

    fn float(&self, value: f64, specifier: u8) -> Vec<u8> {
        let magnitude = value.abs();
        let spec = specifier.to_ascii_lowercase();
        let precision = self.precision.unwrap_or(6);
        let mut digits = if value.is_nan() {
            "nan".to_owned()
        } else if value.is_infinite() {
            "inf".to_owned()
        } else {
            match spec {
                b'f' => format!("{magnitude:.precision$}"),
                b'e' => scientific(magnitude, precision),
                b'g' => {
                    let precision = precision.max(1);
                    // Choose notation using the exponent after rounding to
                    // P significant digits (e.g. %.3g of 999.9 is 1e+03).
                    let scientific = scientific(magnitude, precision - 1);
                    let (_, exponent) = scientific.split_once('e').unwrap();
                    let exponent: i32 = exponent.parse().unwrap();
                    let mut text = if exponent >= -4 && exponent < precision as i32 {
                        let decimals = (precision as i64 - exponent as i64 - 1) as usize;
                        format!("{magnitude:.decimals$}")
                    } else {
                        scientific
                    };
                    if !self.flags.alternate {
                        let end = text.find('e').unwrap_or(text.len());
                        if text[..end].contains('.') {
                            let trimmed = text[..end]
                                .trim_end_matches('0')
                                .trim_end_matches('.')
                                .len();
                            text.replace_range(trimmed..end, "");
                        }
                    }
                    text
                }
                _ => unreachable!(),
            }
        };
        if self.flags.alternate && value.is_finite() && !digits.contains('.') {
            let end = digits.find('e').unwrap_or(digits.len());
            digits.insert(end, '.');
        }
        // Darwin does not print a sign for NaN, even with '+' or a sign bit.
        let prefix = if value.is_nan() {
            ""
        } else {
            self.sign(value.is_sign_negative())
        };
        if specifier.is_ascii_uppercase() {
            digits.make_ascii_uppercase();
        }
        // Infinity and NaN are space-padded even when '0' was requested.
        self.number(prefix, &digits, self.flags.zero && value.is_finite())
    }
}

fn scientific(value: f64, precision: usize) -> String {
    let text = format!("{value:.precision$e}");
    let (mantissa, exponent) = text.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    format!("{mantissa}e{exponent:+03}")
}

/// String formatting for printf and NSLog. Guest arguments are decoded here;
/// shared field formatting handles signs, precision, and width in that order.
///
/// `NS_LOG` enables NSString's `%@` object descriptions. `get_format_char`
/// returns the byte at the given index, or zero at the end of the format.
pub fn printf_inner<const NS_LOG: bool, F: Fn(&Mem, GuestUSize) -> u8>(
    env: &mut Environment,
    get_format_char: F,
    mut args: VaList,
) -> Vec<u8> {
    let mut res = Vec::new();
    let mut index = 0usize;
    loop {
        let c = get_format_char(&env.mem, index as GuestUSize);
        index += 1;
        if c == 0 {
            break;
        }
        if c != b'%' {
            res.push(c);
            continue;
        }
        let conversion =
            Conversion::parse(|i| get_format_char(&env.mem, i as GuestUSize), &mut index);
        let field = conversion.resolve(|| args.next::<i32>(env));
        let length = conversion.length;
        let specifier = match conversion.specifier {
            b'D' => b'd',
            b'U' => b'u',
            b'O' => b'o',
            other => other,
        };
        let bytes = match specifier {
            0 => break,
            b'%' => vec![b'%'],
            b'd' | b'i' => {
                let value: i64 = match length {
                    Some("ll") => args.next(env),
                    Some("hh") => args.next::<i8>(env).into(),
                    Some("h") => args.next::<i16>(env).into(),
                    _ => args.next::<i32>(env).into(),
                };
                field.integer(value.unsigned_abs(), value < 0, specifier)
            }
            b'u' | b'o' | b'x' | b'X' => {
                let value: u64 = match length {
                    Some("ll") => args.next(env),
                    Some("hh") => args.next::<u8>(env).into(),
                    Some("h") => args.next::<u16>(env).into(),
                    _ => args.next::<u32>(env).into(),
                };
                field.integer(value, false, specifier)
            }
            b'p' => {
                let ptr: MutVoidPtr = args.next(env);
                field.integer(ptr.to_bits().into(), false, b'p')
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                // ARM iOS long double has the same representation as double.
                field.float(args.next::<f64>(env), specifier)
            }
            b'c' if length != Some("l") => field.text(&[args.next::<u8>(env)]),
            b'c' | b'C' => {
                let c = if specifier == b'C' {
                    args.next::<unichar>(env).into()
                } else {
                    args.next::<wchar_t>(env) as u32
                };
                // TODO: support isolated UTF-16 surrogates in NSString formats.
                let c = char::from_u32(c).unwrap();
                field.text(c.encode_utf8(&mut [0; 4]).as_bytes())
            }
            b's' | b'S' => {
                if length == Some("l") || specifier == b'S' {
                    let ctype_locale = setlocale(env, LC_CTYPE, Ptr::null());
                    assert_eq!(env.mem.read(ctype_locale), b'C');
                    let ptr: ConstPtr<wchar_t> = args.next(env);
                    let text = if ptr.is_null() {
                        "(null)".to_owned()
                    } else {
                        env.mem.wcstr_at(ptr)
                    };
                    let mut end = field.precision.unwrap_or(text.len()).min(text.len());
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    field.text(&text.as_bytes()[..end])
                } else {
                    let ptr: ConstPtr<u8> = args.next(env);
                    let mut text = Vec::new();
                    if ptr.is_null() {
                        text.extend_from_slice(b"(null)");
                    } else if let Some(precision) = field.precision {
                        // %.Ns need not point to a null-terminated string.
                        for i in 0..precision {
                            let c = env.mem.read(ptr + i as GuestUSize);
                            if c == 0 {
                                break;
                            }
                            text.push(c);
                        }
                    } else {
                        text.extend_from_slice(env.mem.cstr_at(ptr));
                    }
                    if let Some(precision) = field.precision {
                        text.truncate(precision);
                    }
                    field.text(&text)
                }
            }
            b'@' if NS_LOG => {
                let object: id = args.next(env);
                let description: id = msg![env; object description];
                let text = if description == nil {
                    "(null)".to_owned()
                } else {
                    ns_string::to_rust_string(env, description).into_owned()
                };
                field.text(text.as_bytes())
            }
            b'a' | b'A' | b'n' => unimplemented!("printf %{}", specifier as char),
            _ => {
                // Undefined conversions on Darwin drop '%' and print the
                // unknown character. Leave it for the next iteration.
                index -= 1;
                continue;
            }
        };
        res.extend_from_slice(&bytes);
    }
    log_dbg!("=> {:?}", std::str::from_utf8(&res));
    res
}

#[cfg(test)]
mod formatting_tests {
    use super::*;

    fn conversion(format: &str, arguments: &[i32]) -> (Conversion, FieldFormat) {
        assert!(format.starts_with('%'));
        let mut index = 1;
        let conversion = Conversion::parse(
            |i| format.as_bytes().get(i).copied().unwrap_or(0),
            &mut index,
        );
        assert_eq!(index, format.len());
        let mut arguments = arguments.iter();
        let field = conversion.resolve(|| *arguments.next().unwrap());
        assert!(arguments.next().is_none());
        (conversion, field)
    }

    fn integer(format: &str, value: i64, arguments: &[i32]) -> String {
        let (conversion, field) = conversion(format, arguments);
        String::from_utf8(field.integer(value.unsigned_abs(), value < 0, conversion.specifier))
            .unwrap()
    }

    fn float(format: &str, value: f64) -> String {
        let (conversion, field) = conversion(format, &[]);
        String::from_utf8(field.float(value, conversion.specifier)).unwrap()
    }

    #[test]
    fn heavy_mach_upgrade_labels() {
        // The crashed call's CFString at 0x6bdbc points to the first format.
        // Exercise the same parser and fields used by NSString formatting;
        // the %@ argument here is its already-resolved object description.
        for (format, name) in [("%@ : %d(%+d)", "Attack"), ("Size : %d(%+d)", "")] {
            for (change, expected) in [(5, "+5"), (0, "+0"), (-5, "-5")] {
                let mut result = Vec::new();
                let mut values = [20i64, change].into_iter();
                let mut index = 0;
                while index < format.len() {
                    let c = format.as_bytes()[index];
                    index += 1;
                    if c != b'%' {
                        result.push(c);
                        continue;
                    }
                    let conversion = Conversion::parse(
                        |i| format.as_bytes().get(i).copied().unwrap_or(0),
                        &mut index,
                    );
                    let field = conversion.resolve(|| panic!("unexpected '*'"));
                    let bytes = if conversion.specifier == b'@' {
                        field.text(name.as_bytes())
                    } else {
                        let value = values.next().unwrap();
                        field.integer(value.unsigned_abs(), value < 0, conversion.specifier)
                    };
                    result.extend(bytes);
                }
                let label = if name.is_empty() { "Size" } else { name };
                assert_eq!(
                    String::from_utf8(result).unwrap(),
                    format!("{label} : 20({expected})")
                );
            }
        }
    }

    #[test]
    fn signs_width_and_precision() {
        for (format, value, expected) in [
            ("%+d", 5, "+5"),
            ("%+d", 0, "+0"),
            ("%+i", -5, "-5"),
            ("%+6d", 5, "    +5"),
            ("%+06d", 5, "+00005"),
            ("%06d", -5, "-00005"),
            ("%+06d", 0, "+00000"),
            ("%+08.4d", -5, "   -0005"),
            ("%+08.4d", 5, "   +0005"),
            ("%-+08.4d", 5, "+0005   "),
            ("%0+ -8.4d", 5, "+0005   "),
            ("% d", 5, " 5"),
            ("% +d", 5, "+5"),
            ("% 06d", 5, " 00005"),
            ("%.0d", 0, ""),
            ("%+.0d", 0, "+"),
            ("%+4.0d", 0, "   +"),
            ("%+.3d", -5, "-005"),
            ("%+#.2d", 5, "+05"),
            ("%+lld", i64::MIN, "-9223372036854775808"),
        ] {
            assert_eq!(integer(format, value, &[]), expected, "{format}, {value}");
        }
        assert_eq!(integer("%+0*d", 5, &[-6]), "+5    ");
        assert_eq!(integer("%+0*.*d", 5, &[6, -1]), "+00005");
        assert_eq!(integer("%+0*.*d", 5, &[-8, 4]), "+0005   ");
    }

    #[test]
    fn unsigned_radices_and_text_fields() {
        for (format, value, expected) in [
            ("%llu", u64::MAX, "18446744073709551615"),
            ("%llX", u64::MAX, "FFFFFFFFFFFFFFFF"),
            ("%#08x", 42, "0x00002a"),
            ("%#08.4X", 42, "  0X002A"),
            ("%-#8x", 42, "0x2a    "),
            ("%#x", 0, "0"),
            ("%#.0x", 0, ""),
            ("%#o", 8, "010"),
            ("%#.0o", 0, "0"),
            ("%#.4o", 8, "0010"),
            ("%+#u", 5, "5"),
            ("%08p", 42, "0x00002a"),
            ("%p", 0, "0x0"),
        ] {
            let (conversion, field) = conversion(format, &[]);
            assert_eq!(
                field.integer(value, false, conversion.specifier),
                expected.as_bytes(),
                "{format}"
            );
        }
        let (_, field) = conversion("%-5s", &[]);
        assert_eq!(field.text(b"abc"), b"abc  ");
        let (_, field) = conversion("%3c", &[]);
        assert_eq!(field.text(&[0xff]), [b' ', b' ', 0xff]);
    }

    #[test]
    fn floating_point_edges() {
        for (format, value, expected) in [
            ("%+08.2f", 5.0, "+0005.00"),
            ("%-+8.2f", 5.0, "+5.00   "),
            ("%+.0f", -0.0, "-0"),
            ("%+#.0F", 5.0, "+5."),
            ("%+.1E", 99.99, "+1.0E+02"),
            ("%.3g", 999.9, "1e+03"),
            ("%.1g", 0.00009999, "0.0001"),
            ("%+#.3G", 5.0, "+5.00"),
            ("%+.3g", 100000.0, "+1e+05"),
            ("%+010.3g", -5.0, "-000000005"),
            ("%+08F", f64::INFINITY, "    +INF"),
            ("%+08f", f64::NEG_INFINITY, "    -inf"),
            ("%+08f", f64::NAN, "     nan"),
            ("%+08F", -f64::NAN, "     NAN"),
        ] {
            assert_eq!(float(format, value), expected, "{format}, {value}");
        }
    }

    #[test]
    fn numeric_formats_match_host_printf() {
        // Independent oracle for combinations and rounding boundaries. Only
        // controlled numeric formats and matching C argument types go to libc.
        let mut buffer = [0u8; 2048];
        for flags in ["", "+", " ", "0", "+0", "-+0", "#", "+#0", " #0", "0- +#"] {
            for width in ["", "1", "20"] {
                for precision in ["", ".0", ".1", ".3", ".8", ".15"] {
                    for spec in [b'd', b'i', b'u', b'o', b'x', b'X'] {
                        let format = format!("%{flags}{width}{precision}ll{}", spec as char);
                        let c_format = std::ffi::CString::new(format.clone()).unwrap();
                        let (_, field) = conversion(&format, &[]);
                        for bits in [0u64, 1, 42, i64::MAX as u64, i64::MIN as u64, u64::MAX] {
                            let signed = matches!(spec, b'd' | b'i');
                            let value = bits as i64;
                            let actual = field.integer(
                                if signed { value.unsigned_abs() } else { bits },
                                signed && value < 0,
                                spec,
                            );
                            // SAFETY: the format has exactly one ll integer
                            // conversion; buffer has the given writable size.
                            let count = unsafe {
                                if signed {
                                    libc::snprintf(
                                        buffer.as_mut_ptr().cast(),
                                        buffer.len(),
                                        c_format.as_ptr(),
                                        value as libc::c_longlong,
                                    )
                                } else {
                                    libc::snprintf(
                                        buffer.as_mut_ptr().cast(),
                                        buffer.len(),
                                        c_format.as_ptr(),
                                        bits as libc::c_ulonglong,
                                    )
                                }
                            };
                            assert!(count >= 0 && (count as usize) < buffer.len());
                            assert_eq!(actual, buffer[..count as usize], "{format}, {bits}");
                        }
                    }
                    for spec in [b'f', b'F', b'e', b'E', b'g', b'G'] {
                        let format = format!("%{flags}{width}{precision}{}", spec as char);
                        let c_format = std::ffi::CString::new(format.clone()).unwrap();
                        for value in [
                            0.0,
                            -0.0,
                            1.5,
                            -42.25,
                            9.9999,
                            999.9,
                            0.00009999,
                            1e-20,
                            1e20,
                            f64::MAX,
                            f64::MIN_POSITIVE,
                            f64::from_bits(1),
                            f64::INFINITY,
                            f64::NEG_INFINITY,
                            f64::NAN,
                        ] {
                            // NaN sign conventions differ between C libraries.
                            if value.is_nan() && !cfg!(target_vendor = "apple") {
                                continue;
                            }
                            // SAFETY: the format has exactly one double
                            // conversion; buffer has the given writable size.
                            let count = unsafe {
                                libc::snprintf(
                                    buffer.as_mut_ptr().cast(),
                                    buffer.len(),
                                    c_format.as_ptr(),
                                    value,
                                )
                            };
                            assert!(count >= 0 && (count as usize) < buffer.len());
                            assert_eq!(
                                float(&format, value).as_bytes(),
                                &buffer[..count as usize],
                                "{format}, {value}"
                            );
                        }
                    }
                }
            }
        }
    }
}

fn snprintf(
    env: &mut Environment,
    dest: MutPtr<u8>,
    n: GuestUSize,
    format: ConstPtr<u8>,
    args: DotDotDot,
) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!("snprintf() implemented as a wrapper of vsnprintf()");

    vsnprintf(env, dest, n, format, args.start())
}

fn vasprintf(
    env: &mut Environment,
    ret: MutPtr<MutPtr<u8>>,
    format: ConstPtr<u8>,
    arg: VaList,
) -> i32 {
    log_dbg!(
        "vasprintf({:?}, {:?} ({:?}), ...)",
        ret,
        format,
        env.mem.cstr_at_utf8(format)
    );

    let res = printf_inner::<false, _>(env, |mem, idx| mem.read(format + idx), arg);
    let count: GuestUSize = (res.len() + 1).try_into().unwrap();

    let dest: MutPtr<u8> = env.mem.alloc(count * guest_size_of::<u8>()).cast();

    let dest_slice = env.mem.bytes_at_mut(dest, count);
    for (i, &byte) in res.iter().chain(b"\0".iter()).enumerate() {
        dest_slice[i] = byte;
    }

    env.mem.write(ret, dest);

    res.len().try_into().unwrap()
}

fn vprintf(env: &mut Environment, format: ConstPtr<u8>, arg: VaList) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!(
        "vprintf({:?} ({:?}), ...)",
        format,
        env.mem.cstr_at_utf8(format)
    );

    let res = printf_inner::<false, _>(env, |mem, idx| mem.read(format + idx), arg);
    // TODO: I/O error handling
    let _ = std::io::stdout().write_all(&res);
    res.len().try_into().unwrap()
}

fn vsnprintf(
    env: &mut Environment,
    dest: MutPtr<u8>,
    n: GuestUSize,
    format: ConstPtr<u8>,
    arg: VaList,
) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!(
        "vsnprintf({:?} {:?} {:?})",
        dest,
        format,
        env.mem.cstr_at_utf8(format)
    );

    let res = printf_inner::<false, _>(env, |mem, idx| mem.read(format + idx), arg);
    if n == 0 {
        return res.len().try_into().unwrap();
    }
    let middle = if ((n - 1) as usize) < res.len() {
        &res[..(n - 1) as usize]
    } else {
        &res[..]
    };

    let dest_slice = env.mem.bytes_at_mut(dest, n);
    for (i, &byte) in middle.iter().chain(b"\0".iter()).enumerate() {
        dest_slice[i] = byte;
    }

    res.len().try_into().unwrap()
}

fn vsprintf(env: &mut Environment, dest: MutPtr<u8>, format: ConstPtr<u8>, arg: VaList) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!(
        "vsprintf({:?}, {:?} ({:?}), ...)",
        dest,
        format,
        env.mem.cstr_at_utf8(format)
    );

    let res = printf_inner::<false, _>(env, |mem, idx| mem.read(format + idx), arg);

    let dest_slice = env
        .mem
        .bytes_at_mut(dest, (res.len() + 1).try_into().unwrap());
    for (i, &byte) in res.iter().chain(b"\0".iter()).enumerate() {
        dest_slice[i] = byte;
    }

    res.len().try_into().unwrap()
}

fn __sprintf_chk(
    env: &mut Environment,
    dest: MutPtr<u8>,
    _flags: i32,
    strlen: GuestUSize,
    format: ConstPtr<u8>,
    args: DotDotDot,
) -> i32 {
    if strlen == 0 {
        panic!();
    }
    // TODO: respect flags level
    // TODO: full overflow check
    sprintf(env, dest, format, args)
}

fn sprintf(env: &mut Environment, dest: MutPtr<u8>, format: ConstPtr<u8>, args: DotDotDot) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!(
        "sprintf({:?}, {:?} ({:?}), ...)",
        dest,
        format,
        env.mem.cstr_at_utf8(format)
    );

    let res = printf_inner::<false, _>(env, |mem, idx| mem.read(format + idx), args.start());

    let dest_slice = env
        .mem
        .bytes_at_mut(dest, (res.len() + 1).try_into().unwrap());
    for (i, &byte) in res.iter().chain(b"\0".iter()).enumerate() {
        dest_slice[i] = byte;
    }

    res.len().try_into().unwrap()
}

fn swprintf(
    env: &mut Environment,
    ws: MutPtr<wchar_t>,
    n: GuestUSize,
    format: ConstPtr<wchar_t>,
    args: DotDotDot,
) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!("swprintf() implemented as a wrapper of vswprintf()");

    vswprintf(env, ws, n, format, args.start())
}

fn vswprintf(
    env: &mut Environment,
    ws: MutPtr<wchar_t>,
    n: GuestUSize,
    format: ConstPtr<wchar_t>,
    args: VaList,
) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    // TODO: support other locales
    let ctype_locale = setlocale(env, LC_CTYPE, Ptr::null());
    assert_eq!(env.mem.read(ctype_locale), b'C');

    let wcstr_format = env.mem.wcstr_at(format);
    log_dbg!(
        "vswprintf({:?}, {}, {:?} ({:?}), ...)",
        ws,
        n,
        format,
        wcstr_format
    );

    let wcstr_format_bytes = wcstr_format.as_bytes();
    let len: GuestUSize = wcstr_format_bytes.len() as GuestUSize;
    let res = printf_inner::<false, _>(
        env,
        |_mem, idx| {
            if idx == len {
                b'\0'
            } else {
                wcstr_format_bytes[idx as usize]
            }
        },
        args,
    );

    let to_write = n.min(res.len() as GuestUSize);
    for i in 0..to_write {
        env.mem.write(ws + i, res[i as usize] as wchar_t);
    }
    if to_write >= n {
        // TODO: set errno
        return -1;
    }
    env.mem.write(ws + to_write, wchar_t::default());
    to_write as i32
}

fn printf(env: &mut Environment, format: ConstPtr<u8>, args: DotDotDot) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!(
        "printf({:?} ({:?}), ...)",
        format,
        env.mem.cstr_at_utf8(format)
    );

    let res = printf_inner::<false, _>(env, |mem, idx| mem.read(format + idx), args.start());
    // TODO: I/O error handling
    let _ = std::io::stdout().write_all(&res);
    res.len().try_into().unwrap()
}

// TODO: more printf variants

/// A simple wrapper around [sscanf_common_generic] for the case of C string.
fn sscanf_common(
    env: &mut Environment,
    src: ConstPtr<u8>,
    format: ConstPtr<u8>,
    args: VaList,
) -> i32 {
    sscanf_common_generic(
        env,
        |env, s, idx| Ok(env.mem.read(s + idx)),
        |_, _, _| (),
        src.cast_mut(),
        format,
        args,
    )
}

/// Formatted scan implementation for `sscanf` family.
///
/// `getc_fn` is a callback to get next character from `subject`.
/// 3rd parameter in this callback is a index which is safe to ignore
/// (for example, in case of a file stream).
/// Error signifies an abnormal stop of input,
/// such as [crate::libc::stdio::EOF] in the file stream.
/// Note: `'\0'` does not necessary expect to produce an error!
///
/// `ungetc_fn` is a callback to un-get character from `subject`.
/// Could be ignored entirely (for example, in case of a string).
///
/// `subject` is either C string or file stream (for now).
///
/// `args` is the list of arguments to store produced outputs.
///
/// TODO: instead of `u8: From<T>` constraint, implement a conversion callback
fn sscanf_common_generic<
    T,
    U,
    F1: Fn(&mut Environment, MutPtr<U>, GuestUSize) -> Result<T, ()>,
    F2: Fn(&mut Environment, MutPtr<U>, u8), // TODO: make last param generic too?
>(
    env: &mut Environment,
    getc_fn: F1,
    ungetc_fn: F2,
    subject: MutPtr<U>,
    format: ConstPtr<u8>,
    mut args: VaList,
) -> i32
where
    u8: From<T>,
{
    let mut src_char_idx = 0;
    let mut format_char_idx = 0;

    let mut matched_args = 0;

    'outer: loop {
        let c = env.mem.read(format + format_char_idx);
        format_char_idx += 1;

        if c == b'\0' {
            break;
        }
        if c != b'%' {
            let mut cc: u8 = getc_fn(env, subject, src_char_idx).unwrap().into(); // TODO: EOF
            if isspace(env, format + format_char_idx - 1) {
                // "any single whitespace character in the format string
                // consumes all available consecutive whitespace characters
                // from the input"
                while isspace_inner(cc) {
                    src_char_idx += 1;
                    cc = getc_fn(env, subject, src_char_idx).unwrap().into(); // TODO: EOF
                }
                // backtrack one
                ungetc_fn(env, subject, cc);
                continue;
            }
            if c != cc {
                return matched_args;
            }
            src_char_idx += 1;
            continue;
        }

        let mut max_width: u32 = 0;
        while let c @ b'0'..=b'9' = env.mem.read(format + format_char_idx) {
            max_width = max_width * 10 + (c - b'0') as u32;
            format_char_idx += 1;
        }

        let length_modifier = match env.mem.read(format + format_char_idx) {
            b'h' => {
                format_char_idx += 1;
                if env.mem.read(format + format_char_idx) == b'h' {
                    format_char_idx += 1;
                    Some("hh")
                } else {
                    Some("h")
                }
            }
            b'l' => {
                format_char_idx += 1;
                if env.mem.read(format + format_char_idx) == b'l' {
                    format_char_idx += 1;
                    Some("ll")
                } else {
                    Some("l")
                }
            }
            // q seems to be an equivalent of 'll'
            // https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Strings/Articles/formatSpecifiers.html#//apple_ref/doc/uid/TP40004265-SW1
            b'q' => {
                format_char_idx += 1;
                Some("ll")
            }

            _ => None,
        };

        let specifier = env.mem.read(format + format_char_idx);
        format_char_idx += 1;

        if !b"[cn".contains(&specifier) {
            // skip whitespaces
            let x = getc_fn(env, subject, src_char_idx);
            if x.is_err() {
                break 'outer;
            }
            let mut cc: u8 = x.unwrap().into();
            while isspace_inner(cc) {
                src_char_idx += 1;
                let x = getc_fn(env, subject, src_char_idx);
                if x.is_err() {
                    break 'outer;
                }
                cc = x.unwrap().into();
            }
            // backtrack one
            ungetc_fn(env, subject, cc);
        }

        match specifier {
            b'd' | b'i' => {
                let base: u32 = if specifier == b'd' {
                    10
                } else {
                    // automatic base detection in strtol
                    0
                };

                let res = str_to_int_inner_generic(
                    env,
                    &getc_fn,
                    &ungetc_fn,
                    subject,
                    src_char_idx,
                    base,
                    if max_width > 0 { max_width } else { u32::MAX },
                    |s, base| {
                        // We parse as i64 to accommodate for
                        // `signed long long` case (reserved for future use).
                        // Later, the actual conversion to target type is
                        // happening when we actually write value back.
                        i64::from_str_radix(s, base).unwrap()
                    },
                    |num| num.checked_mul(-1).unwrap(),
                );
                match res {
                    Ok((val, len)) => {
                        src_char_idx += len;
                        match length_modifier {
                            Some("hh") => {
                                let c_char_ptr: ConstPtr<i8> = args.next(env);
                                env.mem.write(c_char_ptr.cast_mut(), val as i8);
                            }
                            Some("h") => {
                                let c_short_ptr: ConstPtr<i16> = args.next(env);
                                env.mem.write(c_short_ptr.cast_mut(), val as i16);
                            }
                            None => {
                                let c_int_ptr: ConstPtr<i32> = args.next(env);
                                env.mem.write(c_int_ptr.cast_mut(), val as i32);
                            }
                            _ => unimplemented!("length_modifier {:?}", length_modifier),
                        }
                    }
                    Err(_) => break,
                }
            }
            b'f' | b'g' => {
                assert_eq!(max_width, 0); // TODO
                let res = atof_inner_generic(env, &getc_fn, &ungetc_fn, subject, src_char_idx);
                let val = match res {
                    Ok((val, len)) => {
                        src_char_idx += len;
                        val
                    }
                    Err(_) => break,
                };
                match length_modifier {
                    None => {
                        let c_int_ptr: ConstPtr<f32> = args.next(env);
                        env.mem.write(c_int_ptr.cast_mut(), val as f32);
                    }
                    Some("l") => {
                        let c_int_ptr: ConstPtr<f64> = args.next(env);
                        env.mem.write(c_int_ptr.cast_mut(), val);
                    }
                    Some(modifier) => {
                        unimplemented!("Length formater '{}' for f", modifier)
                    }
                }
            }
            b'x' | b'X' | b'u' => {
                let base: u32 = match specifier {
                    b'x' | b'X' => 16,
                    b'u' => 10,
                    _ => unreachable!(),
                };
                let res = str_to_int_inner_generic(
                    env,
                    &getc_fn,
                    &ungetc_fn,
                    subject,
                    src_char_idx,
                    base,
                    if max_width > 0 { max_width } else { u32::MAX },
                    |s, base| {
                        // We parse as u64 to accommodate for
                        // `unsigned long long` case (reserved for future use).
                        // Later, the actual conversion to target type is
                        // happening when we actually write value back.
                        u64::from_str_radix(s, base).unwrap()
                    },
                    |num| num.wrapping_neg(),
                );
                match res {
                    Ok((val, len)) => {
                        src_char_idx += len;
                        match length_modifier {
                            Some("hh") => {
                                let c_char_ptr: ConstPtr<u8> = args.next(env);
                                env.mem.write(c_char_ptr.cast_mut(), val as u8);
                            }
                            Some("h") => {
                                let c_short_ptr: ConstPtr<u16> = args.next(env);
                                env.mem.write(c_short_ptr.cast_mut(), val as u16);
                            }
                            None => {
                                let c_u32_ptr: ConstPtr<u32> = args.next(env);
                                env.mem.write(c_u32_ptr.cast_mut(), val as u32);
                            }
                            Some("ll") => {
                                let c_u64_ptr: ConstPtr<u64> = args.next(env);
                                env.mem.write(c_u64_ptr.cast_mut(), val);
                            }
                            _ => unimplemented!("length_modifier {:?}", length_modifier),
                        }
                    }
                    Err(_) => break,
                }
            }
            b'[' => {
                assert!(length_modifier.is_none());
                // [set] case
                assert_ne!(env.mem.read(format + format_char_idx), b']');
                let mut c: u8;
                let inverted = if env.mem.read(format + format_char_idx) == b'^' {
                    format_char_idx += 1;
                    assert_ne!(env.mem.read(format + format_char_idx), b']');
                    true
                } else {
                    false
                };
                // Build set
                let mut set: HashSet<u8> = HashSet::new();
                c = env.mem.read(format + format_char_idx);
                format_char_idx += 1;
                while c != b']' {
                    if env.mem.read(format + format_char_idx) == b'-' {
                        assert_ne!(env.mem.read(format + format_char_idx + 1), b']');
                        let cc = env.mem.read(format + format_char_idx + 1);
                        for x in c..=cc {
                            set.insert(x);
                        }
                        format_char_idx += 2;
                    } else {
                        set.insert(c);
                    }
                    c = env.mem.read(format + format_char_idx);
                    format_char_idx += 1;
                }
                let mut dst_ptr: MutPtr<u8> = args.next(env);
                let mut matched = false;
                // Consume `src` while chars are in the set
                // (or not in the set if inverted)
                let mut cc = getc_fn(env, subject, src_char_idx).unwrap().into(); // TODO: EOF
                src_char_idx += 1;
                let mut match_count = 0;
                while set.contains(&cc) ^ inverted && cc != b'\0' {
                    matched = true;
                    env.mem.write(dst_ptr, cc);
                    dst_ptr += 1;
                    match_count += 1;
                    if max_width > 0 && match_count == max_width {
                        break;
                    }
                    cc = getc_fn(env, subject, src_char_idx).unwrap().into(); // TODO: EOF
                    src_char_idx += 1;
                }
                if !(set.contains(&cc) ^ inverted && cc != b'\0') {
                    // We need to backtrack one position
                    ungetc_fn(env, subject, cc);
                    src_char_idx -= 1;
                }
                if matched {
                    env.mem.write(dst_ptr, b'\0');
                } else {
                    matched_args -= 1;
                }
            }
            b's' => {
                assert!(length_modifier.is_none());
                let orig_dst_ptr: MutPtr<u8> = args.next(env);
                let mut dst_ptr: MutPtr<u8> = orig_dst_ptr;
                let mut written = 0;
                loop {
                    let x = getc_fn(env, subject, src_char_idx);
                    if x.is_err() {
                        break;
                    }
                    let cc: u8 = x.unwrap().into();
                    if !isspace_inner(cc) {
                        if cc == b'\0' {
                            break;
                        }
                        if max_width > 0 && written >= max_width {
                            ungetc_fn(env, subject, cc);
                            break;
                        }
                        env.mem.write(dst_ptr, cc);
                        src_char_idx += 1;
                        dst_ptr += 1;
                        written += 1;
                    } else {
                        ungetc_fn(env, subject, cc);
                        break;
                    }
                }
                env.mem.write(dst_ptr, b'\0');
                log_dbg!(
                    "sscanf_common_generic read %s '{:?}'",
                    env.mem.cstr_at_utf8(orig_dst_ptr)
                );
            }
            // TODO: more specifiers
            _ => unimplemented!("Format character '{}'", specifier as char),
        }

        matched_args += 1;
    }

    matched_args
}

fn sscanf(env: &mut Environment, src: ConstPtr<u8>, format: ConstPtr<u8>, args: DotDotDot) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!(
        "sscanf({:?} ({:?}), {:?} ({:?}), ...)",
        src,
        env.mem.cstr_at_utf8(src),
        format,
        env.mem.cstr_at_utf8(format)
    );

    sscanf_common(env, src, format, args.start())
}

fn swscanf(
    env: &mut Environment,
    ws: ConstPtr<wchar_t>,
    format: ConstPtr<wchar_t>,
    args: DotDotDot,
) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    // TODO: support other locales
    let ctype_locale = setlocale(env, LC_CTYPE, Ptr::null());
    assert_eq!(env.mem.read(ctype_locale), b'C');

    let w_string = env.mem.wcstr_at(ws);
    let w_format = env.mem.wcstr_at(format);
    log_dbg!(
        "swscanf({:?} ({:?}), {:?} ({:?}), ...)",
        ws,
        w_string,
        format,
        w_format
    );
    // TODO: refactor code to parametrise sscanf_common()
    // for normal and wide strings instead
    let c_string = env.mem.alloc_and_write_cstr(w_string.as_bytes());
    let c_format = env.mem.alloc_and_write_cstr(w_format.as_bytes());
    let res = sscanf(env, c_string.cast_const(), c_format.cast_const(), args);
    env.mem.free(c_string.cast());
    env.mem.free(c_format.cast());
    res
}

fn vsscanf(env: &mut Environment, src: ConstPtr<u8>, format: ConstPtr<u8>, arg: VaList) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!(
        "vsscanf({:?}, {:?} ({:?}), ...)",
        src,
        format,
        env.mem.cstr_at_utf8(format)
    );

    sscanf_common(env, src, format, arg)
}

fn fscanf(
    env: &mut Environment,
    stream: MutPtr<FILE>,
    format: ConstPtr<u8>,
    args: DotDotDot,
) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!(
        "fscanf({:?}, {:?} ({:?}), ...)",
        stream,
        format,
        env.mem.cstr_at_utf8(format)
    );

    let cc = getc(env, stream);
    if cc == EOF {
        return EOF;
    } else {
        assert_eq!(cc, ungetc(env, cc, stream));
    }

    sscanf_common_generic(
        env,
        |env, file, _idx| {
            let c = getc(env, file);
            if c == EOF {
                Err::<u8, ()>(())
            } else {
                Ok(<i32 as TryInto<u8>>::try_into(c).unwrap())
            }
        },
        |env, file, c| {
            assert_eq!(c as i32, ungetc(env, c as i32, file));
        },
        stream,
        format,
        args.start(),
    )
}

fn fprintf(
    env: &mut Environment,
    stream: MutPtr<FILE>,
    format: ConstPtr<u8>,
    args: DotDotDot,
) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!("fprintf() implemented as a wrapper of vfprintf()");

    vfprintf(env, stream, format, args.start())
}

fn vfprintf(env: &mut Environment, stream: MutPtr<FILE>, format: ConstPtr<u8>, arg: VaList) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!(
        "vfprintf({:?}, {:?} ({:?}), ...)",
        stream,
        format,
        env.mem.cstr_at_utf8(format)
    );

    let res = printf_inner::<false, _>(env, |mem, idx| mem.read(format + idx), arg);
    // TODO: I/O error handling
    match env.mem.read(stream).fd {
        STDIN_FILENO => panic!("Unexpected file descriptor"),
        STDOUT_FILENO => _ = std::io::stdout().write_all(&res),
        STDERR_FILENO => _ = std::io::stderr().write_all(&res),
        _ => {
            let buf = env.mem.alloc_and_write_cstr(res.as_slice());
            let result = fwrite(
                env,
                buf.cast_const().cast(),
                1,
                res.len() as GuestUSize,
                stream,
            );
            assert_eq!(result, res.len() as GuestUSize);
            env.mem.free(buf.cast());
        }
    }
    res.len().try_into().unwrap()
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(sscanf(_, _, _)),
    export_c_func!(swscanf(_, _, _)),
    export_c_func!(vsscanf(_, _, _)),
    export_c_func!(fscanf(_, _, _)),
    export_c_func!(snprintf(_, _, _, _)),
    export_c_func!(vasprintf(_, _, _)),
    export_c_func!(vprintf(_, _)),
    export_c_func!(vsnprintf(_, _, _, _)),
    export_c_func!(vsprintf(_, _, _)),
    export_c_func!(__sprintf_chk(_, _, _, _, _)),
    export_c_func!(sprintf(_, _, _)),
    export_c_func!(swprintf(_, _, _, _)),
    export_c_func!(vswprintf(_, _, _, _)),
    export_c_func!(printf(_, _)),
    export_c_func!(fprintf(_, _, _)),
    export_c_func!(vfprintf(_, _, _)),
];

// Helper function, not a part of printf family
// TODO: write proper libc's isspace()
pub fn isspace(env: &mut Environment, src: ConstPtr<u8>) -> bool {
    let c = env.mem.read(src);
    isspace_inner(c)
}
pub fn isspace_inner(c: u8) -> bool {
    // Rust's definition of whitespace excludes vertical tab, unlike C's
    c.is_ascii_whitespace() || c == b'\x0b'
}
