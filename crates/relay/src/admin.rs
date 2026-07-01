//! Admin CLI for managing relay user accounts.

use crate::api::create_user_account;
use crate::db::Db;
use clap::Subcommand;
use std::path::Path;

#[derive(Subcommand, Debug)]
pub enum UserCmd {
    /// Create a user account (email + password)
    Add {
        email: String,
        #[arg(long)]
        password: String,
        #[arg(long, default_value = "")]
        name: String,
    },
    /// List all accounts
    List,
    /// Delete an account by email
    Del { email: String },
}

pub fn run(cmd: UserCmd, db_path: &Path) -> anyhow::Result<()> {
    let db = Db::open(db_path)?;
    match cmd {
        UserCmd::Add {
            email,
            password,
            name,
        } => {
            let user = create_user_account(&db, &email, &password, &name)?;
            println!("Created {} ({})", user.email, user.name);
        }
        UserCmd::List => {
            let users = db.list_users()?;
            if users.is_empty() {
                println!("No users.");
            } else {
                for u in users {
                    println!("{:<40} {}", u.email, u.name);
                }
            }
        }
        UserCmd::Del { email } => {
            let email = email.trim().to_lowercase();
            if db.delete_user_by_email(&email)? {
                println!("Deleted {email}");
            } else {
                anyhow::bail!("user not found: {email}");
            }
        }
    }
    Ok(())
}
