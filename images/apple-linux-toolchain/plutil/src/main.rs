use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    io::{self, Write},
    path::PathBuf,
};

use apple_plist::{Format, Value};

const VERSION: &str = "once-plutil 0.1.0";

enum OutputFormat {
    Json,
    Xml,
    Binary,
}

struct Options {
    format: OutputFormat,
    output: PathBuf,
    input: PathBuf,
}

fn main() {
    if let Err(error) = run(env::args_os().skip(1).collect()) {
        eprintln!("plutil: {error}");
        std::process::exit(1);
    }
}

fn run(args: Vec<OsString>) -> Result<(), Box<dyn Error>> {
    if args.iter().any(|arg| arg == "--help" || arg == "-help") {
        print_help();
        return Ok(());
    }
    if args.iter().any(|arg| arg == "--version") {
        println!("{VERSION}");
        return Ok(());
    }

    let options = parse_options(args)?;
    let input = fs::read(&options.input)?;
    let value: Value = apple_plist::from_slice(&input)?;
    let output = match options.format {
        OutputFormat::Json => serde_json::to_vec_pretty(&value)?,
        OutputFormat::Xml => apple_plist::to_vec(&value, Format::Xml)?,
        OutputFormat::Binary => apple_plist::to_vec(&value, Format::Binary)?,
    };

    if options.output.as_os_str() == "-" {
        io::stdout().write_all(&output)?;
        io::stdout().write_all(b"\n")?;
    } else {
        fs::write(options.output, output)?;
    }
    Ok(())
}

fn parse_options(args: Vec<OsString>) -> Result<Options, String> {
    let mut arguments = args.into_iter();
    let mut conversion = None;
    let mut output = None;
    let mut input = None;

    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("-convert") => conversion = Some(next_value(&mut arguments, "-convert")?),
            Some("-o") => output = Some(PathBuf::from(next_value(&mut arguments, "-o")?)),
            Some(value) if value.starts_with('-') => {
                return Err(format!("unsupported option: {value}"));
            }
            _ if input.is_none() => input = Some(PathBuf::from(argument)),
            _ => return Err("exactly one input path is required".to_owned()),
        }
    }

    let format = match conversion.as_deref().and_then(|value| value.to_str()) {
        Some("json") => OutputFormat::Json,
        Some("xml1") => OutputFormat::Xml,
        Some("binary1") => OutputFormat::Binary,
        Some(value) => return Err(format!("unsupported conversion: {value}")),
        None => return Err("-convert is required".to_owned()),
    };
    let output = output.ok_or_else(|| "-o is required".to_owned())?;
    let input = input.ok_or_else(|| "an input path is required".to_owned())?;

    Ok(Options {
        format,
        output,
        input,
    })
}

fn next_value(
    arguments: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<OsString, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn print_help() {
    println!("Usage: plutil -convert <json|xml1|binary1> -o <path|-> <input>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_xml_property_lists_to_json() {
        let source = br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Name</key><string>Once</string></dict></plist>"#;
        let value: Value = apple_plist::from_slice(source).unwrap();
        let json = serde_json::to_string(&value).unwrap();

        assert_eq!(json, r#"{"Name":"Once"}"#);
    }

    #[test]
    fn accepts_the_xcode_project_conversion_invocation() {
        let options = parse_options(vec![
            "-convert".into(),
            "json".into(),
            "-o".into(),
            "-".into(),
            "project.pbxproj".into(),
        ])
        .unwrap();

        assert!(matches!(options.format, OutputFormat::Json));
        assert_eq!(options.output, PathBuf::from("-"));
        assert_eq!(options.input, PathBuf::from("project.pbxproj"));
    }
}
