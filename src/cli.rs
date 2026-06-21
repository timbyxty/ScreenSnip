use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "screensnip", about = "ScreenSnip — скриншот области в буфер обмена")]
pub struct Cli {
    /// Интерактивный выбор области (откроется оверлей)
    #[arg(long)]
    pub region: bool,

    /// Захват всего экрана (без оверлея)
    #[arg(long)]
    pub fullscreen: bool,

    /// Путь для сохранения скриншота (PNG). Без этого флага — в буфер обмена
    #[arg(short, long)]
    pub output: Option<String>,

    /// Копировать в буфер обмена (по умолчанию для --region)
    #[arg(short, long)]
    pub clipboard: bool,

    /// Задержка перед захватом (секунд)
    #[arg(short, long, default_value = "0")]
    pub delay: u32,
}

impl Cli {
    pub fn is_cli_mode(&self) -> bool {
        self.region || self.fullscreen
    }
}
