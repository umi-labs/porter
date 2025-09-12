use anyhow::Result;
use dialoguer::{Select, Input, Confirm, MultiSelect};
use std::io;
use colored::Colorize;

/// Prompts the user for input with a given message
pub fn prompt(message: &str) -> Result<String> {
    Ok(Input::new().with_prompt(message).interact_text()?)
}

/// Prompts for user input with a default value
pub fn prompt_with_default(message: &str, default: &str) -> Result<String> {
    let input = Input::<String>::new()
        .with_prompt(message)
        .default(default.to_string())
        .interact()?;
    Ok(input)
}

/// Prompts the user for confirmation with a yes/no question
pub fn confirm(message: &str) -> Result<bool> {
    confirm_with_default(message, false)
}

/// Prompts the user for confirmation with a yes/no question and custom default
pub fn confirm_with_default(message: &str, default: bool) -> Result<bool> {
    Ok(Confirm::new()
        .with_prompt(message)
        .default(default)
        .interact()?)
}

/// Prompts the user to select an option from a list
pub fn select<T: AsRef<str> + std::fmt::Display>(message: &str, options: &[T]) -> Result<usize> {
    select_with_default(message, options, 0)
}

/// Prompts the user to select an option from a list with a default selection
pub fn select_with_default<T: AsRef<str> + std::fmt::Display>(message: &str, options: &[T], default: usize) -> Result<usize> {
    Ok(Select::new()
        .with_prompt(message)
        .items(options)
        .default(default)
        .interact()?)
}

/// Prompts the user to edit a string
pub fn edit(message: &str, initial: &str) -> Result<String> {
    println!("{}", message);
    println!("Current value: {}", initial);
    prompt("Enter new value (leave empty to keep current)")
        .map(|input| if input.is_empty() { initial.to_string() } else { input })
}

/// Prompts the user to select an option from a list using arrow keys with mapping display
pub fn select_with_arrows<T: AsRef<str> + std::fmt::Display>(message: &str, options: &[T], target_field: &str) -> Result<usize> {
    if options.is_empty() {
        return Err(anyhow::anyhow!("No options provided to select_with_arrows"));
    }
    
    // Reorder options to put "Skip this field" at the top
    let mut reordered_options = Vec::new();
    let mut skip_option = None;
    
    for option in options {
        if option.as_ref() == "Skip this field" {
            skip_option = Some(option.as_ref().to_string());
        } else {
            reordered_options.push(option.as_ref().to_string());
        }
    }
    
    // Add skip option at the top if it exists
    if let Some(skip) = skip_option {
        reordered_options.insert(0, skip);
    }
    
    // Calculate the maximum width of source field names for alignment
    let max_width = reordered_options
        .iter()
        .filter(|option| *option != "Skip this field")
        .map(|option| option.len())
        .max()
        .unwrap_or(0);
    
    // Format options to show the mapping relationship with colors and alignment
    let formatted_options: Vec<String> = reordered_options
        .iter()
        .map(|option| {
            if option == "Skip this field" {
                format!("⏭️  {}", option.cyan().bold())
            } else {
                let padding = " ".repeat(max_width.saturating_sub(option.len()));
                format!("→ {} {} > {}", option.blue(), padding, target_field.green().bold())
            }
        })
        .collect();
    
    // Use dialoguer's Select with pagination (10 items per page)
    let selection = Select::new()
        .with_prompt(message)
        .items(&formatted_options)
        .default(0)
        .max_length(10)
        .interact()?;
    
    Ok(selection)
}

/// Displays a mapping operation with color-coded indicators
pub fn display_mapping_operation(operation: &str, source: &str, target: &str) {
    let operation_symbol = match operation {
        "direct" => "→".cyan(),
        "point" => "📍".yellow(),
        "split" => "✂️".magenta(),
        "custom" => "⚙️".red(),
        _ => "→".cyan(),
    };
    println!("  {} {} > {}", operation_symbol, source.green(), target.blue());
}

/// Displays a summary of mapping operations for confirmation
pub fn display_mapping_summary(mappings: &[(String, String, String)]) {
    println!("\n{}", "=== Mapping Summary ===".yellow().bold());
    println!("{}", "The following field mappings will be created:".white().bold());
    println!();
    
    for (operation, source, target) in mappings {
        display_mapping_operation(operation, source, target);
    }
    println!();
}

/// Prompts user to confirm mapping operations
pub fn confirm_mappings(mappings: &[(String, String, String)]) -> Result<bool> {
    display_mapping_summary(mappings);
    
    let options = vec![
        format!("{} {}", "✓".green(), "Apply all mappings".green()),
        format!("{} {}", "✗".red(), "Cancel and review".red()),
        format!("{} {}", "🔄".yellow(), "Start over".yellow())
    ];
    let selection = Select::new()
        .with_prompt("Do you want to proceed with these mappings?")
        .items(&options)
        .default(0)
        .interact()?;
    
    match selection {
        0 => Ok(true),   // Apply all mappings
        1 => Ok(false),  // Cancel
        2 => Err(io::Error::new(io::ErrorKind::Interrupted, "User chose to start over").into()),
        _ => Ok(false),
    }
}

/// Prompts the user to multi-select items from a list. Returns the indices selected.
pub fn multi_select<T: AsRef<str> + std::fmt::Display>(message: &str, options: &[T], preselect_all: bool) -> Result<Vec<usize>> {
    let mut ms = MultiSelect::new();
    ms.with_prompt(message).items(options);
    if preselect_all {
        let defaults: Vec<bool> = options.iter().map(|_| true).collect();
        ms.defaults(&defaults);
    }
    Ok(ms.interact()?)
}
