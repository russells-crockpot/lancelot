// TODO:
//   - get completion actually working
//   - formatting

extern crate log;
extern crate simplelog;

use goblin::pe::PE;
use goblin::Object;
use log::{debug, error, info, trace};
use rayon::prelude::*;
use std::env;
use std::fs;
use std::io::prelude::*;
use zydis;

pub struct Config {
    pub filename: String,
}

impl Config {
    pub fn from_args(args: env::Args) -> Result<Config, &'static str> {
        let args: Vec<String> = args.collect();

        if args.len() < 2 {
            return Err("not enough arguments");
        }

        let filename = args[1].clone();
        trace!("config: parsed filename: {:?}", filename);

        Ok(Config { filename })
    }
}

pub fn setup_logging(_args: &Config) {
    simplelog::TermLogger::init(simplelog::LevelFilter::Info, simplelog::Config::default())
        .expect("failed to setup logging");
}

#[derive(Debug)]
pub enum Error {
    FileAccess,
    FileFormat,
    NotImplemented,
}

fn align(i: usize, b: usize) -> usize {
    let rem = i % b;
    if rem == 0 {
        i
    } else {
        i + (b - rem)
    }
}

pub fn hexdump_ascii(b: u8) -> char {
    if b.is_ascii_graphic() || b == b' ' {
        b as char
    } else {
        '.'
    }
}

pub fn hexdump(buf: &[u8], offset: usize) -> String {
    // 01234567:  00 01 02 03 04 05 06 07  ...............
    // <prefix>   <hex col>                <ascii col>

    let padding = "  ";

    let padding_size = 2;
    let hex_col_size = 3;
    let ascii_col_size = 1;
    let prefix_size = 8 + 1;
    let newline_size = 1;
    let line_size = prefix_size
        + padding_size
        + 16 * hex_col_size
        + padding_size
        + 16 * ascii_col_size
        + newline_size;
    let line_count = align(buf.len(), 0x10) / 0x10;

    let mut ret = String::with_capacity(line_count * line_size);

    let mut line = String::with_capacity(line_size);
    let mut remaining_count = buf.len();
    for line_index in 0..line_count {
        let line_elem_count = 0x10.min(remaining_count);
        let padding_elem_count = 0x10 - line_elem_count;

        // 01234567:  00 01 02 03 04 05 06 07  ...............
        // ^^^^^^^^^
        line.push_str(format!("{:08x}:", offset + 0x10 * line_index).as_str());

        // 01234567:  00 01 02 03 04 05 06 07  ...............
        //          ^^
        line.push_str(padding);

        // 01234567:  00 01 02 03 04 05 06 07  ...............
        //            ^^^
        for elem in &buf[line_index..line_index + line_elem_count] {
            line.push_str(format!("{:02x} ", elem).as_str());
        }
        for _ in 0..padding_elem_count {
            line.push_str("   ");
        }

        // 01234567:  00 01 02 03 04 05 06 07  ...............
        //                                   ^^
        line.push_str(padding);

        // 01234567:  00 01 02 03 04 05 06 07  ...............
        //                                     ^
        for elem in &buf[line_index..line_index + line_elem_count] {
            line.push(hexdump_ascii(*elem))
        }
        for _ in 0..padding_elem_count {
            line.push(' ');
        }
        line.push_str(padding);

        // 01234567:  00 01 02 03 04 05 06 07  ...............
        //                                                    ^
        line.push('\n');

        ret.push_str(line.as_str());
        line.truncate(0x0);
        remaining_count -= line_elem_count;
    }

    ret
}

fn foo(pe: &PE, buf: &[u8]) -> Result<(), Error> {
    info!("foo: {}", pe.name.unwrap_or("(unknown)"));

    info!("bitness: {}", if pe.is_64 { "64" } else { "32" });
    info!("image base: 0x{:x}", pe.image_base);
    info!("entry rva: 0x{:x}", pe.entry);

    // like:
    //
    //     sections:
    //       - .text
    //         raw size:     0x18aa00
    //         virtual size: 0x18a9a8
    info!("sections:");
    for section in pe.sections.iter() {
        if section.real_name.is_some() {
            info!(
                "  - {} ({})",
                String::from_utf8_lossy(&section.name[..]),
                section
                    .real_name
                    .as_ref()
                    .unwrap_or(&"(unknown)".to_string())
            );
        } else {
            info!("  - {}", String::from_utf8_lossy(&section.name[..]));
        }

        info!("    raw size:     0x{:x}", section.size_of_raw_data);
        info!("    virtual size: 0x{:x}", section.virtual_size);

        // TODO: figure out if we will work with usize, or u64, or what, then assert usize is ok.
        // `usize::max_value()`
        let mut secbuf = vec![0; align(section.virtual_size as usize, 0x200)];

        {
            let secsize = section.size_of_raw_data as usize;
            let rawbuf = &mut secbuf[..secsize];
            let pstart = section.pointer_to_raw_data as usize;
            rawbuf.copy_from_slice(&buf[pstart..pstart + secsize]);
        }

        info!(
            "\n{}",
            hexdump(
                &secbuf[..0x1C],
                pe.image_base + section.virtual_address as usize
            )
        );

        let decoder =
            zydis::Decoder::new(zydis::MachineMode::Long64, zydis::AddressWidth::_64).unwrap();
        let insns: Vec<_> = secbuf
            .par_windows(0x10)
            .map(|ibuf| decoder.decode(ibuf))
            .collect();

        info!("total instructions: {}", insns.len());

        info!(
            "successful disassembles: {}",
            insns.par_iter().filter(|insn| insn.is_ok()).count()
        );

        info!(
            "valid instructions: {}",
            insns
                .par_iter()
                .filter(|insn| match insn {
                    Ok(Some(_)) => true,
                    _ => false,
                })
                .count()
        );
    }

    Ok(())
}

fn read_file(filename: &str) -> Result<Vec<u8>, Error> {
    debug!("read_file: {:?}", filename);

    let mut buf = Vec::new();
    {
        debug!("reading file: {}", filename);
        let mut f = match fs::File::open(filename) {
            Ok(f) => f,
            Err(_) => {
                error!("failed to open file: {}", filename);
                return Err(Error::FileAccess);
            }
        };
        let bytes_read = match f.read_to_end(&mut buf) {
            Ok(c) => c,
            Err(_) => {
                error!("failed to read entire file: {}", filename);
                return Err(Error::FileAccess);
            }
        };
        debug!("read {} bytes", bytes_read);
        if bytes_read < 0x10 {
            error!("file too small: {}", filename);
            return Err(Error::FileFormat);
        }
    }

    Ok(buf)
}

pub struct Workspace<'a> {
    pub filename: String,
    pub buf: &'a [u8],
    pub obj: Object<'a>,
}

impl<'a> Workspace<'a> {
    pub fn from_buf(filename: &str, buf: &'a [u8]) -> Result<Workspace<'a>, Error> {
        let obj = match Object::parse(buf) {
            Ok(o) => o,
            Err(e) => {
                error!("failed to parse file: {} error: {:?}", filename, e);
                return Err(Error::FileFormat);
            }
        };

        match &obj {
            Object::PE(_) => {
                info!("found PE file");
            }
            Object::Elf(_) => {
                error!("found ELF file, format not yet supported");
                return Err(Error::NotImplemented);
            }
            Object::Mach(_) => {
                error!("found Mach-O file, format not yet supported");
                return Err(Error::NotImplemented);
            }
            Object::Archive(_) => {
                error!("found archive file, format not yet supported");
                return Err(Error::NotImplemented);
            }
            Object::Unknown(_) => {
                error!(
                    "unknown file format, magic: | {:02X} {:02X} | '{}{}' ",
                    buf[0],
                    buf[1],
                    hexdump_ascii(buf[0]),
                    hexdump_ascii(buf[1])
                );
                return Err(Error::NotImplemented);
            }
        }

        Ok(Workspace {
            filename: filename.to_string(),
            buf: buf,
            obj: obj,
        })
    }
}

pub fn run(args: &Config) -> Result<(), Error> {
    debug!("filename: {:?}", args.filename);

    let buf = read_file(&args.filename)?;
    let w = Workspace::from_buf(&args.filename, &buf)?;

    if let Object::PE(pe) = w.obj {
        foo(&pe, w.buf).expect("failed to foo");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use matches::matches;
    use std::path::PathBuf;

    fn get_k32_rsrc() -> Vec<u8> {
        let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        d.push("resources");
        d.push("test");
        d.push("k32.bin");

        let mut buf = read_file(d.to_str().unwrap()).unwrap();
        buf[0] = b'M';
        buf[1] = b'Z';
        buf
    }

    #[test]
    fn test_load_pe() {
        let k32 = get_k32_rsrc();
        let w = Workspace::from_buf("k32.bin", &k32).unwrap();
        assert!(matches!(w.obj, Object::PE(_)));
    }
}
