/// A local notebook command and the text arguments selected in the terminal input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Workflow {
    name: String,
    command: String,
    arguments: Vec<Argument>,
}

impl Workflow {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            arguments: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn arguments(&self) -> &Vec<Argument> {
        &self.arguments
    }

    pub fn with_arguments(mut self, arguments: Vec<Argument>) -> Self {
        self.arguments = arguments;
        self
    }

    pub fn set_name(&mut self, name: &str) {
        name.clone_into(&mut self.name);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct Argument {
    pub name: String,
    pub description: Option<String>,
    pub default_value: Option<String>,
}

impl Argument {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default_value = Some(default.into());
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn description(&self) -> &Option<String> {
        &self.description
    }

    pub fn default_value(&self) -> &Option<String> {
        &self.default_value
    }
}
