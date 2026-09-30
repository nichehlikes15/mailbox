mod email;
mod inbox;
mod mailtopbar;
mod tempmailgenerate;
mod settings;
mod sidebar;
mod topbar;
mod email_view;

pub use email::EmailView;
pub use inbox::Inbox;
pub use mailtopbar::MailTopBar;
pub use tempmailgenerate::Popout;
//pub use settings::Settings;
pub use sidebar::Sidebar;
pub use topbar::TopBar;
pub use email_view::HtmlBody;