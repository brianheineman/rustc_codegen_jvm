# Rust → JVM Toolchain 🚀  

Welcome! This project provides a toolchain for compiling Rust code into Java bytecode, allowing it to run on the JVM.  

## How It Works  

The toolchain follows these steps:  

1. **Parsing & MIR Generation**  
   - Rust code is first parsed and converted into *MIR* (Mid-level Intermediate Representation) using `rustc`.  

2. **MIR to Java Bytecode**  
   - The MIR for each crate is translated into a Java Classfile (containing JVM bytecode) via `rustc_codegen_jvm`, which is the core of this repository.  
   - Source code for this component is located in the `src` folder.  

3. **Linking & `.jar` Generation**  
   - Java Classfiles for all crates used in a library or executable are linked into a single `.jar` file, making it ready to run on the JVM.  
   - This step is handled by `java-linker-rs`, a custom-built linker in this repository (found in the `java-linker` folder).  

## Current Capabilities  

- ✅ Compiling a minimal `no_std` & `no_core` Rust program with an empty `main` function.  
- ✅ Simple mathematical operations on `i32`s: addition, subtraction, and returning `()` or an `i32`.  

### Next Milestone:  
🚧 **Full support for the `core` crate** is in progress!  

## How to Use the Toolchain  

### Prerequisites  
- Ensure you're on the **latest nightly build** of Rust.  
- Install necessary Rust components:  
  ```sh
  rustup component add rust-src rustc-dev llvm-tools-preview
  ```
- Run the setup script:  
  ```sh
  chmod +x setup.sh && ./setup.sh
  ```  

### Compiling Rust to JVM Bytecode  
To use the toolchain for compiling your Rust project:  

1. **Configure Cargo**  
   - In your project, create a `.cargo/config.toml` file and add:  
     ```toml
     [build]
     rustflags = ["-Z", "codegen-backend=/path/to/rustc_codegen_jvm/target/debug/librustc_codegen_jvm.dylib"]
     ```
2. **Update the Target JSON**  
   - Modify `jvm-unknown-unknown.json`, replacing the `"linker"` entry with the absolute path of the `java-linker`:  
     ```
     /path/to/rustc_codegen_jvm/java-linker/target/debug/java-linker
     ```
3. **Build Your Project**  
   ```sh
   cargo build --target /path/to/rustc_codegen_jvm/jvm-unknown-unknown.json
   ```
4. **Find & Run the Generated `.jar`**  
   - The compiled `.jar` file will be in:  
     ```
     target/jvm-unknown-unknown/debug/[cratename].jar
     ```
   - If it's a binary (not a library), run it using:  
     ```sh
     java -jar target/jvm-unknown-unknown/debug/[cratename].jar
     ```  

### Running Tests  
- If you modified the target JSON file, **revert the changes** before running tests.  
- Execute the test script:  
  ```sh
  python3 Tester.py
  ```  
- Look for a **success message** 🎉  
