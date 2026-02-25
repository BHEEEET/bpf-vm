#[derive(Clone, Copy)]
pub struct BpfInst {
    pub opcode: u8,
    pub dst: u8,
    pub src: u8,
    pub offset: i16,
    pub imm: i32,
}

impl BpfInst {
    pub fn from_bytes(bytes: [u8; 8]) -> Self {
        BpfInst {
            opcode: bytes[0],
            dst: bytes[1] & 0x0f,
            src: (bytes[1] >> 4) & 0x0f,
            offset: i16::from_le_bytes([bytes[2], bytes[3]]),
            imm: i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }
}

pub const REGISTER_COUNT: usize = 11;
pub const STACK_SIZE: usize = 512;

pub struct Vm {
    pub regs: [u64; REGISTER_COUNT],
    pub stack: [u8; STACK_SIZE],
    pub pc: usize,
    pub instructions: Vec<BpfInst>,
    pub instruction_limit: u64,
    pub instruction_count: u64,
}

impl Vm {
    pub fn new(instructions: Vec<BpfInst>) -> Self {
        Vm {
            regs: [0; REGISTER_COUNT],
            stack: [0; STACK_SIZE],
            pc: 0,
            instructions,
            instruction_limit: 100000,
            instruction_count: 0,
        }
    }

    pub fn run(&mut self) -> Result<u64, String> {
        while self.pc < self.instructions.len() {
            if self.instruction_count >= self.instruction_limit {
                return Err("Out of gas".to_string());
            }

            let insn = self.instructions[self.pc];
            self.instruction_count += 1;

            match insn.opcode {
                0xb4 | 0xb7 => {
                    self.regs[insn.dst as usize] = insn.imm as u64;
                }

                0x07 => {
                    self.regs[insn.dst as usize] += insn.imm as u64;
                }

                0x95 => {
                    return Ok(self.regs[0]);
                }

                _ => return Err(format!("Unsupported opcode: {}", insn.opcode)),
            }

            self.pc += 1;
        }

        Err("No exit instruction found".to_string())
    }
}
