use anyhow::Result;
use byteorder::{BE, ReadBytesExt};
use clap::{Parser, Subcommand};
use clap_num::maybe_hex;
use std::fs::read;
use std::io::Cursor;
use std::path::PathBuf;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Command,

    /// Input file
    infile: PathBuf,

    /// Output file
    outfile: PathBuf,
}

#[derive(Subcommand)]
enum Command {
    /// Build a font table from an image
    Build {},

    /// Extract a font table to an image
    Extract {
        /// VRAM address of the table
        #[arg(value_parser = maybe_hex::<u32>)]
        vram: u32,

        /// Number of characters in the table
        #[arg(value_parser = maybe_hex::<usize>)]
        num_chars: usize,
    },
}

fn parse_function<T>(cursor: &mut Cursor<T>) -> Result<Option<Box<[u8]>>>
where
    Cursor<T>: ReadBytesExt,
{
    let prologue = (cursor.read_u32::<BE>()?, cursor.read_u32::<BE>()?);
    match prologue {
        (/* lw $s0, 0($a0) */ 0x8C900000, /* addi $a0, $a0, 4 */ 0x20840004) => {
            let mut pixels = vec![0, 0, 0, 0, 0, 0, 0, 0];

            let mut instr;
            while {
                instr = cursor.read_u32::<BE>()?;
                instr != /* jr $s0 */ 0x02000008
            } {
                let offset = instr & 0x0000FFFF;
                pixels[(offset >> 1) as usize] = 0xFF;
            }
            Ok(Some(pixels.into_boxed_slice()))
        }
        (/* lw $s1, 0($sp) */ 0x8FB10000, /* addi $sp, $sp, 4 */ 0x23BD0004) => Ok(None),
        _ => Ok(None),
    }
}

fn extract(data: &[u8], vram: u32, num_chars: usize) -> Result<Vec<u8>> {
    let offsets_len = num_chars * 9 * size_of::<u32>() * 2;

    let data_vram = vram + offsets_len as u32;

    let mut cursor = Cursor::new(&data[..offsets_len]);

    let mut offsets = vec![];
    while let Ok(offset) = cursor.read_u32::<BE>() {
        offsets.push(offset - data_vram)
    }

    let mut cursor = Cursor::new(&data[offsets_len..]);

    let mut font: Vec<u8> = vec![];

    for chunk in offsets.chunks(9).collect::<Vec<_>>().chunks(2) {
        if let [block, _] = chunk {
            for offset in &block[..8] {
                cursor.set_position(*offset as u64);
                if let Some(l) = parse_function(&mut cursor)? {
                    font.extend(l.iter());
                }
            }
        }
    }

    Ok(font)
}

fn main() -> Result<()> {
    let args = Args::parse();

    match args.command {
        Command::Build {} => {
            /*let infile = read_to_string(args.infile)?;
            let mut out = build(&infile)?;
            out.push(0x00);
            if let Some(p) = pad {
                out.resize(p, 0xFF);
            }
            write(args.outfile, out)?;*/
        }
        Command::Extract { vram, num_chars } => {
            let infile = read(args.infile)?;
            let out = extract(&infile, vram, num_chars)?;

            image::save_buffer(
                args.outfile,
                &out,
                8,
                (out.len() / 8) as u32,
                image::ColorType::L8,
            )?;
        }
    }

    Ok(())
}
