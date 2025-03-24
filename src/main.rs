use anyhow::Result;
use byteorder::{BE, ReadBytesExt};
use clap::{Parser, Subcommand};
use clap_num::maybe_hex;
use image::EncodableLayout;
use std::fs::{read, write};
use std::io::Cursor;
use std::path::PathBuf;

const SCREEN_WIDTH: i16 = 640;
type Pixel = u16;

const PROLOGUE: &str = include_str!("prologue.s");
const EPILOGUE: &str = include_str!("epilogue.s");

const ROW_END: &str = include_str!("row_end.s");

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
    Build {
        /// Label for the first part of the table
        first_label: String,

        /// Label for the second part of the table
        second_label: String,
    },

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

fn build_function(row: u8) -> String {
    let mut rv = String::new();

    rv += "    lw     s0, 0(a0)\n";
    rv += &format!("    addi   a0, a0, {}\n", size_of::<u32>());

    for i in 0..u8::BITS {
        if (row >> (u8::BITS - i - 1)) & 1 != 0 {
            rv += &format!("    sh     s1, {}(a1)\n", i * size_of::<Pixel>() as u32);
        }
    }

    rv += "    jr     s0\n";
    rv += &format!(
        "     addi  a1, a1, {}\n",
        SCREEN_WIDTH * size_of::<Pixel>() as i16
    );

    rv
}

fn build(data: &[u8], first_label: &str, second_label: &str) -> Result<String> {
    let mut rv = String::from(PROLOGUE);

    let mut odd = vec![];
    let mut even = vec![];

    for ch in data.chunks_exact(16 * 8) {
        let mut buf = [0; 8 * 2];
        for (index, row) in ch.chunks_exact(8).enumerate() {
            let mut b = 0;

            for i in row {
                b = (b << 1) | (*i != 0) as u8;
            }

            buf[index] = b;
        }

        let o: [u8; 8] = buf[..8].try_into().unwrap();
        let e: [u8; 8] = buf[8..].try_into().unwrap();

        odd.push(o);
        even.push(e);
    }

    let mut odd_rows = vec![];

    for i in 0..(1 << 7) {
        odd_rows.push(((i << 3) & 0b11111000) | ((i >> 4) & 0b00000110));
    }

    let mut extra_rows = vec![];

    let mut odd_strings = vec![];

    for ch in &odd {
        let mut o = vec![];
        for i in ch {
            if !odd_rows.contains(i) {
                o.push(format!("row_extra_{i:08b}"));
                if !extra_rows.contains(i) {
                    extra_rows.push(*i);
                }
            } else {
                o.push(format!("row_odd_{i:08b}"));
            }
        }
        odd_strings.push(o);
    }

    let mut even_rows = vec![];

    for i in 0..(1 << 7) {
        even_rows.push(((i << 3) & 0b11111000) | ((i >> 4) & 0b00000110));
        /*println!(
            "{}",
            format!("{:08b}", even_rows.last().unwrap())
                .replace('0', " ")
                .replace('1', "█")
        );*/
    }

    let mut even_strings = vec![];

    for ch in &even {
        let mut e = vec![];
        for i in ch {
            if !even_rows.contains(i) {
                e.push(format!("row_extra_{i:08b}"));
                if !extra_rows.contains(i) {
                    extra_rows.push(*i);
                }
            } else {
                e.push(format!("row_even_{i:08b}"));
            }
        }
        even_strings.push(e);
    }

    for (index, (o, e)) in odd_strings.iter().zip(&even_strings).enumerate() {
        if index == 0 {
            rv += &format!("EXPORT({})\n", first_label);
        }

        for i in o {
            rv += &format!("    .word {i}\n");
        }

        rv += "    .word row_end\n\n";

        if index == 0 {
            rv += &format!("EXPORT({})\n", second_label);
        }

        for i in e {
            rv += &format!("    .word {i}\n");
        }

        rv += "    .word row_end\n\n";
    }

    for i in odd_rows {
        let name = format!("row_odd_{i:08b}");
        rv += &format!("LEAF({name})\n");
        rv += &build_function(i);
        rv += &format!("END({name})\n\n");
    }

    rv += ROW_END;

    for i in even_rows {
        let name = format!("row_even_{i:08b}");
        rv += &format!("LEAF({name})\n");
        rv += &build_function(i);
        rv += &format!("END({name})\n\n");
    }

    for i in extra_rows {
        let name = format!("row_extra_{i:08b}");
        rv += &format!("LEAF({name})\n");
        rv += &build_function(i);
        rv += &format!("END({name})\n\n");
    }

    rv += EPILOGUE;

    Ok(rv)
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
                let w = instr & 0xFC000000 == 0xAC000000;
                let offset = instr & 0x0000FFFF;
                if w {
                    if pixels[(offset >> 1) as usize] == 0 {
                        pixels[(offset >> 1) as usize] = 0x7F;
                    } else {
                        pixels[(offset >> 1) as usize] = 0xBF;
                    }

                    if pixels[((offset >> 1) + 1) as usize] == 0 {
                        pixels[((offset >> 1) + 1) as usize] = 0x7F;
                    } else {
                        pixels[((offset >> 1) + 1) as usize] = 0xBF;
                    }
                } else {
                    if pixels[(offset >> 1) as usize] == 0 {
                        pixels[(offset >> 1) as usize] = 0xFF;
                    } else {
                        pixels[(offset >> 1) as usize] = 0xBF;
                    }
                }
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
        if let [block, shadow] = chunk {
            for offset in &block[..8] {
                cursor.set_position(*offset as u64);
                if let Some(l) = parse_function(&mut cursor)? {
                    font.extend(l.iter());
                }
            }
            for offset in &shadow[..8] {
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
        Command::Build {
            first_label,
            second_label,
        } => {
            let infile = image::open(args.infile)?;
            assert_eq!(infile.width(), 8);
            assert_eq!(infile.height() % 16, 0);
            let bw = infile.to_luma8();
            let out = build(bw.as_bytes(), &first_label, &second_label)?;

            write(args.outfile, out)?;
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
