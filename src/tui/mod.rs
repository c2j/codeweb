pub mod app;

use crate::error::Result;

pub fn run(project_path: &std::path::Path) -> Result<()> {
    let mut proj = crate::project::Project::find(project_path)?;

    if proj.load_store().is_err() {
        eprintln!("No store found. Running initial analysis...");
        proj.analyze()?;
    }

    let mut terminal = ratatui::init();
    let app = app::App::new(proj);
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}
