/*
    Copyright 2021 Volt Contributors

    Licensed under the Apache License, Version 2.0 (the "License");
    you may not use this file except in compliance with the License.
    You may obtain a copy of the License at

        http://www.apache.org/licenses/LICENSE-2.0

    Unless required by applicable law or agreed to in writing, software
    distributed under the License is distributed on an "AS IS" BASIS,
    WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
    See the License for the specific language governing permissions and
    limitations under the License.
*/

mod commands;

use std::process::exit;

use crate::commands::AppCommand;

use anyhow::Result;
use colored::Colorize;
use tokio::time::Instant;
use volt_core::VERSION;
use volt_utils::{app::App, ERROR_TAG};

#[tokio::main]
async fn main() {
    if let Err(err) = try_main().await {
        eprintln!("{} {}", ERROR_TAG.clone(), err);
        let err_chain = err.chain().skip(1);
        if err_chain.clone().next().is_some() {
            eprintln!("{}", "\nCaused by:".italic().truecolor(190, 190, 190));
        }
        err_chain.for_each(|cause| eprintln!(" - {}", cause.to_string().truecolor(190, 190, 190)));

        #[cfg(not(debug_assertions))]
        eprintln!(
            "\nIf the problem persists, please submit an issue on the Github repository.\n{}",
            "https://github.com/voltpkg/volt/issues/new".underline()
        );
        std::process::exit(1);
    }
}

async fn try_main() -> Result<()> {
    let app = App::initialize();
    let cmd = AppCommand::current().unwrap_or(AppCommand::Script); // Default command is help

    if app.has_flag(&["--help", "-h"]) {
        println!("{}", cmd.help());
        return Ok(());
    }

    if app.has_flag(&["--version"]) {
        println!(
            "volt v{}{}",
            "::".bright_magenta(),
            VERSION.bright_green().bold()
        );
        exit(0);
    }

    let time = Instant::now();
    cmd.run(app).await?;
    println!("Finished in {:.2}s", time.elapsed().as_secs_f32());

    Ok(())
}
