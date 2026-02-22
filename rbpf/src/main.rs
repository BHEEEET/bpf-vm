use elf::endian::AnyEndian;
use elf::ElfBytes;
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let program_data = fs::read("program.so")?;

    let file = ElfBytes::<AnyEndian>::minimal_parse(&program_data)?;

    let section_header = file
        .section_header_by_name(".text")
        .ok()
        .flatten()
        .ok_or("No .text section found")?;

    let prog = file
        .section_data(&section_header)
        .map(|(data, _)| data.to_vec())
        .expect("Failed to get section data");

    let vm = rbpf::EbpfVmNoData::new(Some(&prog))?;
    let result = vm.execute_program()?;
    println!("Program returned: {}", result);

    Ok(())
}
