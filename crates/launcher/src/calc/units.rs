// SPDX-License-Identifier: GPL-3.0-or-later
//! Offline conversion of one arithmetic value between compatible units.

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dimension {
    Length,
    Area,
    Mass,
    Volume,
    Time,
    Speed,
    Data,
}

#[derive(Clone, Copy)]
enum Scale {
    Linear(Dimension, f64),
    Temperature(Temperature),
}

#[derive(Clone, Copy)]
enum Temperature {
    Celsius,
    Fahrenheit,
    Kelvin,
}

impl Temperature {
    const fn absolute_zero(self) -> f64 {
        match self {
            Self::Celsius => -273.15,
            Self::Fahrenheit => -459.67,
            Self::Kelvin => 0.0,
        }
    }
}

struct Unit {
    label: &'static str,
    names: &'static [&'static str],
    scale: Scale,
}

impl Unit {
    const fn linear(label: &'static str, names: &'static [&'static str], dimension: Dimension, scale: f64) -> Self {
        Self { label, names, scale: Scale::Linear(dimension, scale) }
    }

    const fn temperature(label: &'static str, names: &'static [&'static str], scale: Temperature) -> Self {
        Self { label, names, scale: Scale::Temperature(scale) }
    }

    fn parse(text: &str) -> Option<&'static Self> {
        UNITS.iter().find(|unit| unit.label == text || unit.names.iter().any(|name| text.eq_ignore_ascii_case(name)))
    }
}

use Dimension::*;
use Temperature::*;

const INCH: f64 = 0.0254;
const FOOT: f64 = 0.3048;
const MILE: f64 = 1609.344;
const POUND: f64 = 0.453_592_37;
const US_GALLON: f64 = 3.785_411_784;
const IMP_GALLON: f64 = 4.546_09;

const UNITS: &[Unit] = &[
    Unit::linear("mm", &["millimeter", "millimeters", "millimetre", "millimetres"], Length, 0.001),
    Unit::linear("cm", &["centimeter", "centimeters", "centimetre", "centimetres"], Length, 0.01),
    Unit::linear("m", &["meter", "meters", "metre", "metres"], Length, 1.0),
    Unit::linear("km", &["kilometer", "kilometers", "kilometre", "kilometres"], Length, 1000.0),
    Unit::linear("in", &["inch", "inches"], Length, INCH),
    Unit::linear("ft", &["foot", "feet"], Length, FOOT),
    Unit::linear("yd", &["yard", "yards"], Length, 0.9144),
    Unit::linear("mi", &["mile", "miles"], Length, MILE),
    Unit::linear("cm²", &["cm2", "cm^2", "square centimeters"], Area, 0.0001),
    Unit::linear("m²", &["m2", "m^2", "square meters", "square metres"], Area, 1.0),
    Unit::linear("km²", &["km2", "km^2", "square kilometers"], Area, 1_000_000.0),
    Unit::linear("ft²", &["ft2", "ft^2", "square feet"], Area, 0.092_903_04),
    Unit::linear("ha", &["hectare", "hectares"], Area, 10_000.0),
    Unit::linear("acre", &["acres"], Area, 4_046.856_422_4),
    Unit::linear("mg", &["milligram", "milligrams"], Mass, 0.000_001),
    Unit::linear("g", &["gram", "grams"], Mass, 0.001),
    Unit::linear("kg", &["kilogram", "kilograms"], Mass, 1.0),
    Unit::linear("oz", &["ounce", "ounces"], Mass, POUND / 16.0),
    Unit::linear("lb", &["lb", "lbs", "pound", "pounds"], Mass, POUND),
    Unit::linear("mL", &["ml", "milliliter", "milliliters", "millilitre", "millilitres"], Volume, 0.001),
    Unit::linear("L", &["l", "liter", "liters", "litre", "litres"], Volume, 1.0),
    Unit::linear("US fl oz", &["fl oz", "us fl oz"], Volume, US_GALLON / 128.0),
    Unit::linear("US gal", &["gal", "gallon", "gallons", "us gal", "us gallon", "us gallons"], Volume, US_GALLON),
    Unit::linear("imp gal", &["uk gal", "imperial gallon", "imperial gallons", "imp gal"], Volume, IMP_GALLON),
    Unit::linear("ms", &["millisecond", "milliseconds"], Time, 0.001),
    Unit::linear("s", &["sec", "second", "seconds"], Time, 1.0),
    Unit::linear("min", &["minute", "minutes"], Time, 60.0),
    Unit::linear("h", &["hr", "hour", "hours"], Time, 3600.0),
    Unit::linear("d", &["day", "days"], Time, 86_400.0),
    Unit::linear("m/s", &["meters per second", "metres per second"], Speed, 1.0),
    Unit::linear("km/h", &["kph", "kmh", "kilometers per hour", "kilometres per hour"], Speed, 1000.0 / 3600.0),
    Unit::linear("mph", &["mi/h", "miles per hour"], Speed, MILE / 3600.0),
    Unit::linear("kn", &["knot", "knots"], Speed, 1852.0 / 3600.0),
    Unit::linear("b", &["bit", "bits"], Data, 1.0),
    Unit::linear("B", &["byte", "bytes"], Data, 8.0),
    Unit::linear("kb", &["kbit", "kilobit", "kilobits"], Data, 1000.0),
    Unit::linear("kB", &["KB", "kilobyte", "kilobytes"], Data, 8000.0),
    Unit::linear("Mb", &["Mbit", "megabit", "megabits"], Data, 1_000_000.0),
    Unit::linear("MB", &["megabyte", "megabytes"], Data, 8_000_000.0),
    Unit::linear("Gb", &["Gbit", "gigabit", "gigabits"], Data, 1_000_000_000.0),
    Unit::linear("GB", &["gigabyte", "gigabytes"], Data, 8_000_000_000.0),
    Unit::linear("KiB", &["kibibyte", "kibibytes"], Data, 8192.0),
    Unit::linear("MiB", &["mebibyte", "mebibytes"], Data, 8_388_608.0),
    Unit::linear("GiB", &["gibibyte", "gibibytes"], Data, 8_589_934_592.0),
    Unit::temperature("°C", &["c", "°c", "celsius"], Celsius),
    Unit::temperature("°F", &["f", "°f", "fahrenheit"], Fahrenheit),
    Unit::temperature("K", &["k", "kelvin"], Kelvin),
];

pub(super) fn convert(expression: &str) -> Option<(f64, &'static str)> {
    let (source, destination) = split_conversion(expression)?;
    let (amount, source) = split_amount(source)?;
    let value = super::arithmetic(amount, true)?;
    let converted = match (source.scale, destination.scale) {
        (Scale::Linear(from, ratio), Scale::Linear(to, divisor)) if from == to => value * (ratio / divisor),
        (Scale::Temperature(from), Scale::Temperature(to)) => convert_temperature(value, from, to)?,
        _ => return None,
    };
    converted.is_finite().then_some((converted, destination.label))
}

fn split_conversion(expression: &str) -> Option<(&str, &'static Unit)> {
    // Scan from the destination so the first "in" in "12 in in cm" is inches.
    expression.match_indices(char::is_whitespace).rev().find_map(|(index, _)| {
        let (source, rest) = expression.split_at(index);
        let (separator, destination) = rest.trim_start().split_once(char::is_whitespace)?;
        if !separator.eq_ignore_ascii_case("in") && !separator.eq_ignore_ascii_case("to") {
            return None;
        }
        Some((source.trim_end(), Unit::parse(destination.trim())?))
    })
}

fn split_amount(source: &str) -> Option<(&str, &'static Unit)> {
    source.char_indices().skip(1).find_map(|(index, ch)| {
        if !ch.is_alphabetic() && ch != '°' {
            return None;
        }
        let (amount, unit) = source.split_at(index);
        Some((amount.trim_end(), Unit::parse(unit)?))
    })
}

fn convert_temperature(value: f64, source: Temperature, destination: Temperature) -> Option<f64> {
    let absolute_zero = source.absolute_zero();
    if value <= absolute_zero {
        // Keep the boundary reusable even when the conversion formula rounds below it.
        return (value == absolute_zero).then(|| destination.absolute_zero());
    }
    // Use per-pair offsets to keep common fixed points such as 32 °F exact.
    Some(match (source, destination) {
        (Celsius, Fahrenheit) => value.mul_add(9.0 / 5.0, 32.0),
        (Fahrenheit, Celsius) => (value - 32.0) * (5.0 / 9.0),
        (Celsius, Kelvin) => value + 273.15,
        (Kelvin, Celsius) => value - 273.15,
        (Fahrenheit, Kelvin) => (value + 459.67) * (5.0 / 9.0),
        (Kelvin, Fahrenheit) => value.mul_add(9.0 / 5.0, -459.67),
        _ => value,
    })
}
