use elf::endian::AnyEndian;
use elf::ElfBytes;
use std::env;
use std::fs;
use zbpf::{BpfInst, Vm};

fn hardcoded_program() -> Vec<BpfInst> {
    vec![
        BpfInst {
            opcode: 0xb7,
            dst: 0,
            src: 0,
            offset: 0,
            imm: 10,
        },
        BpfInst {
            opcode: 0x07,
            dst: 0,
            src: 0,
            offset: 0,
            imm: 32,
        },
        BpfInst {
            opcode: 0x95,
            dst: 0,
            src: 0,
            offset: 0,
            imm: 0,
        },
    ]
}

fn load_program_from_file(path: &str) -> Result<Vec<BpfInst>, String> {
    let program_data = fs::read(path).map_err(|e| e.to_string())?;

    let file = ElfBytes::<AnyEndian>::minimal_parse(&program_data).map_err(|e| e.to_string())?;

    let section_header = file
        .section_header_by_name(".text")
        .ok()
        .flatten()
        .ok_or("No .text section found")?;

    let text_section = file
        .section_data(&section_header)
        .map(|(data, _)| data.to_vec())
        .expect("Failed to get section data");

    let mut instructions = Vec::new();
    for chunk in text_section.chunks(8) {
        if chunk.len() == 8 {
            let bytes: [u8; 8] = chunk.try_into().unwrap();
            instructions.push(BpfInst::from_bytes(bytes));
        }
    }

    Ok(instructions)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();

    let instructions = if args.len() > 1 && args[1] == "--hardcoded" {
        hardcoded_program()
    } else {
        load_program_from_file("program.so")?
    };

    let mut vm = Vm::new(instructions);

    let result = vm.run()?;
    println!("Program returned: {}", result);

    Ok(())
}
