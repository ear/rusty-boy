use ncurses::*;
use std::collections::HashMap;
use super::language;
use super::language::*;
use super::dbglanguage;
use std;
use super::super::cpu::*;
use super::super::disasm::*;
use std::thread::sleep;
use std::time::Duration;
use std::collections::BTreeSet;


static ASM_WINDOW_HEIGHT: i32 = 3;
static ASM_WINDOW_WIDTH: i32 = 10;
const WIN_Y_DIV: i32 = 5;
const WIN_Y_ADJ: i32 = 2;
const WIN_X_DIV: i32 = 4;
const WIN_X_ADJ: i32 = 3;

#[derive(PartialEq)]
enum DebuggerState {
    Running,
    Paused,
}

pub struct Debugger {
    cpu: Cpu,
    //   symbol_table: HashMap<&'str, Expression>,
    asm_win: WINDOW,
    reg_win: WINDOW,
    in_win: WINDOW,
    dissassembled_rom: Vec<(String, u16)>,
    input_buffer: String,
    output_buffer: Vec<String>,
    debugger_state: DebuggerState,
    watchpoints: BTreeSet<u16>,
    breakpoints: BTreeSet<u16>,
    history_location: usize, // used for scrolling back in history
}

impl Debugger {
    pub fn new(file_name: &str) -> Debugger {
        use ncurses::*;

        let mut max_x = 0;
        let mut max_y = 0;
        initscr();
        cbreak();

        keypad(stdscr(), true);
        echo();

        getmaxyx(stdscr(), &mut max_y, &mut max_x);
        printw(format!("X: {} Y: {}", max_x, max_y).as_ref());

        let mut cpu = Cpu::new();
        cpu.load_rom(file_name);

        let mut romcp = [0u8; 0x8000];
        for i in 0..0x7FFF {
            romcp[i] = cpu.mem[i] as u8;
        }

        let dbg = Debugger {
            cpu: cpu,
            //         symbol_table: HashMap::new(),
            asm_win: create_win((max_y / WIN_Y_DIV) * WIN_Y_ADJ,
                                (max_x / WIN_X_DIV) * WIN_X_ADJ,
                                0,
                                max_x / WIN_X_DIV),
            reg_win: create_win((max_y / WIN_Y_DIV) * WIN_Y_ADJ, max_x / WIN_X_DIV, 0, 0),
            in_win: create_win(max_y - ((max_y / WIN_Y_DIV) * WIN_Y_ADJ),
                               max_x,
                               (max_y / WIN_Y_DIV) * WIN_Y_ADJ,
                               0),
            dissassembled_rom: disasm_rom_to_vec(romcp, 0x7FF0),
            input_buffer: String::new(),
            output_buffer: vec![String::new(), String::new()], // to make history nicer
            debugger_state: DebuggerState::Paused,
            breakpoints: BTreeSet::new(),
            watchpoints: BTreeSet::new(),
            history_location: 0,
        };

        refresh();

        dbg
    }

    pub fn handle_input(&mut self) {

        let mut ch = getch();
        timeout(-1); //make input blocking
        match ch {
            KEY_LEFT => {
                self.cpu.dispatch_opcode();
            }
            // numbers and letters
            v @ 0x20...0x7F => {
                self.input_buffer.push_str(String::from_utf8(vec![v as u8]).unwrap().as_ref())
            }
            // This shouldn't need to be handled
            KEY_RESIZE => (),
            // backspace
            0x8 | KEY_BACKSPACE => {
                self.input_buffer.pop();
            }
            KEY_DL => {
                // Contents modified, no longer historical string
                self.input_buffer.clear();
            }

            KEY_UP => {
                let next_hist = self.history_location;
                let mstr = self.get_nth_hist_item(next_hist);

                if let Some(str) = mstr {
                    self.input_buffer = str;
                    self.history_location += 1;
                }
            }

            KEY_DOWN => {
                let next_hist = if self.history_location == 0 {
                    0
                } else {
                    self.history_location - 1
                };
                let mstr = self.get_nth_hist_item(next_hist as usize);

                if let Some(str) = mstr {
                    self.input_buffer = str;
                    self.history_location = next_hist as usize;
                }
            }

            // Enter (on linux)
            0xA => {
                // do parsing
                let parseret = match dbglanguage::parse_Input(self.input_buffer.as_ref()) {
                    Ok(v) => Ok(v),
                    Err(e) => Err(format!("{:?}", e)),
                };

                let parseval = match parseret {
                    Ok(v) => self.dispatch_debugger_action(v),
                    Err(e) => e,
                };

                let old_input_string = self.input_buffer.clone();
                self.output_buffer.push(old_input_string);
                self.output_buffer.push(parseval);
                self.input_buffer = String::new();

                self.reset_history_location();
            }
            _ => (),
        }
    }

    pub fn refresh_screen(&mut self) {
        wclear(self.asm_win);
        wclear(self.in_win);
        box_(self.in_win, 0, 0);
        box_(self.asm_win, 0, 0);
        box_(self.reg_win, 0, 0);

        //        wscrl(self.reg_win, 5);
        self.draw_registers();
        self.draw_registers16();
        self.draw_watchpoints();
        self.draw_stack_data();
        self.draw_in();
        self.draw_asm();


        wrefresh(self.asm_win);
        wrefresh(self.reg_win);
        wrefresh(self.in_win);

        refresh();
        wrefresh(self.in_win);
    }


    // draws the input window
    fn draw_in(&mut self) {
        let mut x = 0;
        let mut y = 0;
        getmaxyx(self.in_win, &mut y, &mut x);

        let num_lines = y - 3; //number of lines to draw backlog/previous input and output

        let relevant_hist: Vec<(usize, &String)> = self.output_buffer[0..]
            .iter()
            .rev()
            .take(num_lines as usize)
            .enumerate()
            .collect();

        for &(i, hist) in relevant_hist.iter() {
            wmove(self.in_win, num_lines - (i as i32), 1);
            wprintw(self.in_win, hist.as_ref());
        }

        wmove(self.in_win, y - 2, 1);
        wprintw(self.in_win, self.input_buffer.as_ref());
    }

    fn draw_asm(&mut self) {
        let cur_pc = self.cpu.pc;
        let ar_max = self.dissassembled_rom.len() - 1;
        let idx = binsearch_inst(&self.dissassembled_rom,
                                 cur_pc,
                                 0,
                                 ar_max as usize)
            .expect(format!("INVALID INSTRUCTION at {}", self.cpu.pc)
                .as_ref()) as u16;

        if idx > 7 {
            for i in 0..7 {
                let (cur_inst, _) = self.dissassembled_rom[(idx - (7 - i)) as usize].clone();
                let cur_instref = cur_inst.as_ref();
                self.draw_instruction((i + 1) as i32, cur_instref);
            }
            start_color();
            init_pair(1, COLOR_RED, COLOR_BLACK);

            wattron(self.asm_win, COLOR_PAIR(1));
            // highlight current inst
            let (cur_inst, _) = self.dissassembled_rom[idx as usize].clone();
            let cur_instref = cur_inst.as_ref();
            self.draw_instruction(8, cur_instref);
            wattroff(self.asm_win, COLOR_PAIR(1));

            for i in 9..17 {
                let (cur_inst, _) = self.dissassembled_rom[(idx + (i - 8)) as usize].clone();
                let cur_instref = cur_inst.as_ref();
                self.draw_instruction(i as i32, cur_instref);
            }
        } else {
            // not enough instructions before
            let (cur_inst, _) = self.dissassembled_rom[idx as usize].clone();
            let cur_instref = cur_inst.as_ref();
            start_color();			/* Start color 			*/
            init_pair(1, COLOR_RED, COLOR_BLACK);

            // highlight current inst
            wattron(self.asm_win, COLOR_PAIR(1));
            self.draw_instruction(1, cur_instref);
            wattroff(self.asm_win, COLOR_PAIR(1));

            for i in 1..16 {
                let (cur_inst, _) = self.dissassembled_rom[(idx + i) as usize].clone();
                let cur_instref = cur_inst.as_ref();
                self.draw_instruction((i + 1) as i32, cur_instref);

            }
        }


        //        self.dissassembled_rom;

    }

    fn draw_instruction(&mut self, y_loc: i32, disinst: &str) {
        wmove(self.asm_win, y_loc, 1);
        wprintw(self.asm_win, format!("{}", disinst).as_ref());
    }

    // TODO: make this nicer later
    fn draw_watchpoints(&mut self) {
        let mut x = 0;
        let mut y = 0;
        static x_width: i32 = 11;
        getmaxyx(self.reg_win, &mut y, &mut x);
        let watchpoints: Vec<u16> = self.watchpoints.iter().cloned().collect();

        static y_offset: i32 = 9;

        for i in y_offset..(y_offset + (self.watchpoints.len() as i32)) {
            wmove(self.reg_win, i, 1);
            wprintw(self.reg_win,
                    format!("({:X}): {:X}",
                            watchpoints[(i - y_offset) as usize],
                            self.cpu.mem[(watchpoints[(i - y_offset) as usize]) as usize])
                        .as_ref());
        }

    }

    fn draw_register(&mut self, y_loc: i32, name: &str, reg: CpuRegister) {
        wmove(self.reg_win, y_loc, 1);
        wprintw(self.reg_win,
                format!("{:4}: 0x{:02X}",
                        name,
                        self.cpu
                            .access_register(reg)
                            .expect("invalid register"))
                    .as_ref());
    }

    fn draw_register16(&mut self, y_loc: i32, name: &str, reg: CpuRegister16) {
        wmove(self.reg_win, y_loc, 13);
        wprintw(self.reg_win,
                format!("{:2}: 0x{:04X}",
                        name,
                        self.cpu
                            .access_register16(reg))
                    .as_ref());
    }

    fn draw_stack_data(&mut self) {
        static stack_x_pos_offset: i32 = 26;

        let mut x = 0;
        let mut y = 0;
        getmaxyx(self.reg_win, &mut y, &mut x);

        // -2 for top and bottom box lines
        //        let space_available = (y - 2);

        if x < 26 {
            error!("Not enough X space in the register window");
        }

        // div by for 16bit addreses
        let number_of_stack_frames = (0xFFFE - self.cpu.access_register16(CpuRegister16::SP)) / 2;
        let effective_stack_frames = if (number_of_stack_frames as i32) > y {
            y
        } else {
            number_of_stack_frames as i32
        };
        // Stack starts at 0xFFFE
        let mut cur_stack_ptr = 0xFFFE - effective_stack_frames;

        for (i, addr) in (cur_stack_ptr..0xFFFF)
            .filter(|&n| n % 2 == 0)
            .enumerate() {
            // i+1 because enumerate starts at 0
            wmove(self.reg_win, (i as i32) + 1, stack_x_pos_offset);
            wprintw(self.reg_win,
                    format!("({:X}): {:02X}{:02X}",
                            addr,
                            self.cpu.mem[(addr + 1) as usize],
                            self.cpu.mem[addr as usize])
                        .as_ref());
        }
    }

    fn draw_registers(&mut self) {
        static reg8bit_list: [CpuRegister; 8] = [CpuRegister::A,
                                                 CpuRegister::B,
                                                 CpuRegister::C,
                                                 CpuRegister::D,
                                                 CpuRegister::E,
                                                 CpuRegister::H,
                                                 CpuRegister::HL,
                                                 CpuRegister::L];
        static reg8bit_name: [&'static str; 8] = ["A", "B", "C", "D", "E", "H", "(HL)", "L"];

        for i in 0..8 {
            self.draw_register(i + 1, reg8bit_name[i as usize], reg8bit_list[i as usize]);
        }
        wmove(self.reg_win, 8, 1);
        wprintw(self.reg_win,
                format!("{:4}: 0x{:02X}", "F", self.cpu.f).as_ref());

    }

    // draw 16bit registers in the second column (right shifted by 11 characters) of the reg_win
    fn draw_registers16(&mut self) {
        static reg16bit_list: [CpuRegister16; 4] =
            [CpuRegister16::BC, CpuRegister16::DE, CpuRegister16::HL, CpuRegister16::SP];
        static reg16bit_name: [&'static str; 4] = ["BC", "DE", "HL", "SP"];
        for i in 0..4 {
            self.draw_register16(i + 1, reg16bit_name[i as usize], reg16bit_list[i as usize]);
        }

        // 4
        wmove(self.reg_win, 5, 13);
        wprintw(self.reg_win,
                format!("{:2}: 0x{:04X}", "PC", self.cpu.pc).as_ref());
    }

    fn dispatch_debugger_action(&mut self, da: DebuggerAction) -> String {
        match da {
            DebuggerAction::Echo { str: s } => s,
            DebuggerAction::Reset => {
                self.cpu.reset();
                "CPU resetting".to_string()
            }
            DebuggerAction::Run => {
                self.debugger_state = DebuggerState::Running;
                "Running...".to_string()
            }
            DebuggerAction::Step => {
                self.cpu.dispatch_opcode();
                "Stepping...".to_string()
            }
            DebuggerAction::WatchPoint { addr: addr } => {
                self.watchpoints.insert(addr);
                format!("Watchpoint set at 0x{:X}", addr)
            }
            DebuggerAction::UnwatchPoint { addr: addr } => {
                self.watchpoints.remove(&addr);
                format!("Removing watchpoint at 0x{:X}", addr)
            }
            DebuggerAction::SetBreakPoint { addr: addr } => {
                let ar_max = self.dissassembled_rom.len() - 1;
                let bp = binsearch_inst(&self.dissassembled_rom, addr, 0, ar_max as usize);

                if let Some(inst) = bp {
                    self.breakpoints.insert(addr);
                    format!("Setting breakpoint at 0x{:X}", addr)
                } else {
                    format!("Cannot break at invalid address 0x{:X}", addr)
                }
            }
            DebuggerAction::UnsetBreakPoint { addr: addr } => {
                self.breakpoints.remove(&addr);
                format!("Removing breakpoint at 0x{:X}", addr)
            }
            DebuggerAction::RunToAddress { addr: addr } => {
                // will need special case
                "hello".to_string()
            }
            DebuggerAction::Show { show: show } => {
                match show {
                    ShowableThing::Address { addr: addr } => {
                        format!("(0x{:X}) = 0x{:X}", addr, self.cpu.mem[addr as usize])
                    }
                    ShowableThing::Breakpoints => format!("Breakpoints: {:?}", self.breakpoints),
                }
            }
        }
    }

    fn reset_history_location(&mut self) {
        self.history_location = 0;
    }

    //    fn set_history_location(&mut self, val: usize) {}

    fn get_nth_hist_item(&self, val: usize) -> Option<String> {
        // to get the nth item we'll need to adjust by 2*n
        let offset = val * 2;
        let new_idx = match self.output_buffer.len() {
            0 | 1 => panic!("Minimum value of output buffer must be 2"),
            n => {
                if offset > (self.output_buffer.len() - 1) {
                    None
                } else {
                    Some((self.output_buffer.len() - 2) - offset)
                }
            }
        };


        if let Some(x) = new_idx {
            Some(self.output_buffer[x].clone())
        } else {
            None
        }
    }

    pub fn should_run(&self) -> bool {
        self.debugger_state == DebuggerState::Running
    }

    pub fn run(&mut self) {
        let start_time = std::time::Instant::now();

        while start_time.elapsed() < std::time::Duration::from_millis(16) {
            if self.breakpoints.contains(&self.cpu.pc) {
                self.debugger_state = DebuggerState::Paused;
                break;
            }
            self.cpu.dispatch_opcode();
        }

    }

    pub fn make_input_non_blocking(&mut self) {
        timeout(0);
    }

    pub fn no_input(&mut self) -> bool {
        let ch = getch();

        ch == ERR
    }

    pub fn pause(&mut self) {
        self.debugger_state = DebuggerState::Paused;
    }
}

fn create_win(height: i32, width: i32, start_y: i32, start_x: i32) -> WINDOW {
    let win = newwin(height, width, start_y, start_x);
    box_(win, 0, 0);
    wrefresh(win);
    win
}