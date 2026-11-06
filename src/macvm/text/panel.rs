//! The texts of the guided window of `paddock macvm setup`: the six
//! steps of the side panel, their help, the password row, the `Type ...
//! into the guest` buttons and their feedback, and the question for the
//! password that the person set when the guest refuses the one paddock
//! has. Also the terminal lines of that question.

use super::PanelStep;
use crate::macvm::golden::{GUEST_USER, PASSWORD_LEN};

/// The title of the side panel of the guided window.
pub fn panel_title() -> String {
    "Set up paddock's macOS guest".into()
}

/// A step of the guided panel, with a check mark when done.
pub fn panel_step(n: PanelStep, done: bool) -> String {
    let seed;
    let text = match n {
        PanelStep::Language => {
            "Choose your language and region. Keep the keyboard English (US) or ABC, the default: the Type buttons below send the keys of a US keyboard."
        }
        PanelStep::Skips => {
            "Skip every sign-in, analytics and Siri step. paddock needs no Apple ID."
        }
        PanelStep::Account => {
            "Create the account. Full name: paddock, the account name follows. Password: the one below. Click into the Password field of the guest, then click Type the password. Click into Verify and click it again."
        }
        PanelStep::Seed => {
            seed = format!(
                "Install the agent. On the desktop, open Terminal (Finder > Applications > Utilities) and click into it. Click Type the command, then press Return. When sudo asks for the password, click Type the password and press Return. The command:\n{}",
                crate::macvm::seed::command()
            );
            seed.as_str()
        }
        PanelStep::Accessibility => {
            "On the desktop, open System Settings > Privacy & Security > Accessibility and turn on paddock-guest. The guest shows a dialog for it, click Open System Settings."
        }
        PanelStep::Done => "Done. paddock finishes the setup itself (2 minutes).",
    };
    let mark = if done { "✓ " } else { "" };
    format!("{mark}{}. {text}", n.number())
}

/// The `Help` text of a panel step: why the step is there.
pub fn panel_help(n: PanelStep) -> String {
    match n {
        PanelStep::Language => "The guest is a new Mac. Its language and region are yours. The Type buttons send the keys of a US keyboard, so the guest keyboard stays English (US) or ABC until the setup ends. Add your own keyboard in the guest later.",
        PanelStep::Skips => "The guest needs no Apple ID, and an Apple ID does not work in a virtual Mac. Skip it, and skip analytics and Siri.",
        PanelStep::Account => "paddock logs in as paddock in every session. The password stays in paddock's keyring on your Mac. It has only letters that sit on the same key on US, UK, German, French and Chinese keyboards, so it types right whatever keyboard the guest has.",
        PanelStep::Seed => "The command copies paddock-guest and its launchd file from a read-only folder of your Mac into the guest, with the owners that macOS wants, and starts it. paddock on your Mac needs no administrator rights for it. The step is done when the agent dials in. When the command prints FAILED, run it again.",
        PanelStep::Accessibility => "paddock-guest reads the windows of the app in the guest, so each window can show on your desktop.",
        PanelStep::Done => "paddock sets auto login, no sleep and the network of the guest, then seals the guest. Leave the window open until the terminal prints golden ready.",
    }
    .into()
}

/// One numbered help entry of the panel.
pub fn panel_help_entry(n: u8, help: &str) -> String {
    format!("{n}. {help}")
}

/// The `Stop` button and what it keeps.
pub fn panel_stop() -> String {
    "Stop (the setup continues here next time)".into()
}

/// The note under the steps while `Help` is off.
pub fn panel_note() -> String {
    format!(
        "{}\n\nThe account name is {GUEST_USER}. Help shows why each step is there.",
        panel_stop()
    )
}

/// The label over the password that paddock made.
pub fn panel_password_label() -> String {
    format!("Password of the account {GUEST_USER} ({PASSWORD_LEN} lowercase letters):")
}

/// The label over the password after the person typed the one they set.
pub fn panel_password_set_label() -> String {
    format!("Password of the account {GUEST_USER}: the one you set (hidden).")
}

/// The button that copies the password to the clipboard of the Mac.
pub fn panel_copy() -> String {
    "Copy".into()
}

/// The status after `Copy`: the guest has no clipboard before its agent.
pub fn panel_copied() -> String {
    "Copied to the clipboard of your Mac. The guest gets no clipboard before its agent runs: use Type the password.".into()
}

/// The button that types the password into the guest.
pub fn panel_type_password() -> String {
    "Type the password into the guest".into()
}

/// The button that types the install command of step 4 into the guest.
pub fn panel_type_command() -> String {
    "Type the command into the guest".into()
}

/// The hint under the Type buttons.
pub fn panel_type_hint() -> String {
    "First click into the field in the guest, then the button. The buttons send the keys of a US keyboard: the command types right only while the guest keyboard is English (US) or ABC, so keep it during the setup.".into()
}

/// The status while the host types into the guest.
pub fn panel_typing(count: usize) -> String {
    format!("Typing {count} characters into the guest. Do not click or type until it ends.")
}

/// The status after the host typed into the guest.
pub fn panel_typed(count: usize) -> String {
    format!(
        "Typed {count} characters into the guest. If the guest shows other characters, delete them, set the guest keyboard to ABC or English (US) and type again."
    )
}

/// A Type button found a character that no US key gives.
pub fn panel_cannot_type(c: char) -> String {
    format!(
        "paddock cannot type {c:?} with the keys of a US keyboard, so it typed nothing. Type the text by hand."
    )
}

/// A Type button while the host still types.
pub fn panel_typing_busy() -> String {
    "paddock is still typing. Wait until it ends, then click again.".into()
}

/// The question when the guest refused the password at configure.
pub fn panel_wrong_password() -> String {
    format!(
        "The guest refused the password that paddock has. Type the password you set for the account {GUEST_USER} in Setup Assistant, then click Use this password. paddock keeps it in its keyring and runs the configure step again."
    )
}

/// The placeholder of the secure field of that question.
pub fn panel_password_field() -> String {
    format!("The password you set for {GUEST_USER}")
}

/// The button of that question.
pub fn panel_use_password() -> String {
    "Use this password".into()
}

/// The keyring refuses a password: empty, too long or with a control
/// character.
pub fn keyring_password_invalid(max: usize) -> String {
    format!("the password must have 1 to {max} characters and no line break or tab")
}

/// The terminal line when the guest refused the password at configure.
pub fn configure_wrong_password() -> String {
    format!(
        "the guest refused the password of the account {GUEST_USER} (sudo said it is wrong). Type the password you set in Setup Assistant into the paddock window. paddock then runs the configure step again."
    )
}

/// The terminal line after the person typed the password.
pub fn password_replaced() -> String {
    "paddock keeps the password you typed in its keyring and runs the configure step again.".into()
}

/// The reason of a failed state when the guest refused the password.
pub fn password_refused(tries: usize) -> String {
    let times = if tries == 1 {
        "once".to_string()
    } else {
        format!("{tries} times") // not user text
    };
    format!("the guest refused the password of the account {GUEST_USER} {times}")
}

#[cfg(test)]
#[path = "panel_tests.rs"]
mod tests;
