#![feature(alloc_error_hook)]
#![feature(box_patterns)]
#![feature(rustc_private)]
#![warn(clippy::pedantic)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]

//! Rustc Codegen JVM
//!
//! Compiler backend for rustc that generates JVM bytecode, given Rust HIR and MIR.
//! Supports both Rust static libraries and binaries, generating a file - [cratename].class as it's output.
//! The class file supports Java 8 or later.

extern crate rustc_codegen_ssa;
extern crate rustc_data_structures;
extern crate rustc_driver;
extern crate rustc_hir;
extern crate rustc_metadata;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_target;

use rustc_codegen_ssa::{
    CodegenResults, CompiledModule, CrateInfo, ModuleKind, traits::CodegenBackend,
};
use rustc_data_structures::fx::FxIndexMap;
use rustc_metadata::EncodedMetadata;
use rustc_middle::dep_graph::{WorkProduct, WorkProductId};
use rustc_middle::ty::{Instance, Ty, TyCtxt};
use rustc_session::{Session, config::OutputFilenames};
use std::{any::Any, io::Write, path::Path, vec};
use rustc_middle::mir::{
    BasicBlock, BasicBlockData, BinOp, Body, Location, Rvalue, Statement, StatementKind,
    Terminator, TerminatorKind, visit::Visitor,
};
use rustc_codegen_ssa::back::archive::{ArArchiveBuilder, ArchiveBuilder, ArchiveBuilderBuilder};

/// An instance of our Java bytecode codegen backend.
struct MyBackend;

impl CodegenBackend for MyBackend {
    fn locale_resource(&self) -> &'static str {
        ""
    }

    fn codegen_crate<'a>(
        &self,
        tcx: TyCtxt<'_>,
        metadata: EncodedMetadata,
        _need_metadata_module: bool,
    ) -> Box<dyn Any> {
        let mut function_bytecodes = FxIndexMap::default();
        let crate_name = tcx
            .crate_name(rustc_hir::def_id::CRATE_DEF_ID.to_def_id().krate)
            .to_string();

        // Iterate through all items in the crate and find functions
        let module_items = tcx.hir_crate_items(()); // Get ModuleItems
        for item_id in module_items.free_items() {
            // Use free_items() iterator
            let item = tcx.hir_item(item_id);
            if let rustc_hir::ItemKind::Fn {
                ident: i,
                sig: _,
                generics: _,
                body: _,
                has_body: _,
            } = item.kind
            {
                // Corrected destructuring
                let def_id = item_id.owner_id.to_def_id();
                let instance = rustc_middle::ty::Instance::mono(tcx, def_id);
                let mir = tcx.optimized_mir(instance.def_id());

                println!("--- Starting MIR Visitor for function: {i} ---");
                let method_bytecode_instructions: Vec<u8> = Vec::new();
                let function_name = i.to_string(); // Capture function name
                let mut visitor = MirToBytecodeVisitor::new(
                    method_bytecode_instructions,
                    &function_name,
                    tcx,
                    instance,
                ); // Pass tcx and instance
                visitor.visit_body(mir);
                let generated_bytecode = visitor.method_bytecode_instructions;
                println!("--- MIR Visitor Finished for function: {i} ---");

                function_bytecodes.insert(function_name, generated_bytecode); // Store bytecode
            }
        }

        // Generate basic Java bytecode for a class with static methods,
        // passing function_bytecodes which now contains bytecodes for each function
        let bytecode = generate_class_with_static_methods_bytecode(
            crate_name.as_str(),
            &function_bytecodes,
            tcx,
        ); // Modified function to pass tcx

        Box::new((
            bytecode,
            crate_name,
            metadata,
            CrateInfo::new(tcx, "java_bytecode_basic_class".to_string()),
        ))
    }

    fn join_codegen(
        &self,
        ongoing_codegen: Box<dyn Any>,
        _sess: &Session,
        outputs: &OutputFilenames,
    ) -> (CodegenResults, FxIndexMap<WorkProductId, WorkProduct>) {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (bytecode, crate_name, metadata, crate_info) = *ongoing_codegen
                .downcast::<(Vec<u8>, String, EncodedMetadata, CrateInfo)>()
                .expect("in join_codegen: ongoing_codegen is not bytecode vector");

            let class_path = outputs.temp_path_ext("class", None);

            let mut class_file = std::fs::File::create(&class_path)
                .expect("Could not create the Java .class file!");
            class_file
                .write_all(&bytecode)
                .expect("Could not write Java bytecode to file!");

            let modules = vec![CompiledModule {
                name: crate_name,
                kind: ModuleKind::Regular,
                object: Some(class_path),
                bytecode: None,
                dwarf_object: None,
                llvm_ir: None,
                links_from_incr_cache: Vec::new(), // Corrected to Vec::new()
                assembly: None,
            }];
            let codegen_results = CodegenResults {
                modules,
                allocator_module: None,
                metadata_module: None,
                metadata,
                crate_info,
            };
            (codegen_results, FxIndexMap::default())
        }))
        .expect("Could not join_codegen")
    }

    fn link(&self, sess: &Session, codegen_results: CodegenResults, outputs: &OutputFilenames) {
        println!("linking!");

        use rustc_codegen_ssa::back::link::link_binary;
        link_binary(sess, &RlibArchiveBuilder, codegen_results, outputs);
    }    
}

#[unsafe(no_mangle)]
pub extern "Rust" fn __rustc_codegen_backend() -> Box<dyn CodegenBackend> {
    std::alloc::set_alloc_error_hook(custom_alloc_error_hook);
    Box::new(MyBackend)
}

use std::alloc::Layout;
/// # Panics
/// 
/// Panics when called, every time, with a message statating the memory allocation of the bytes
/// corresponding to the provided layout failed.
pub fn custom_alloc_error_hook(layout: Layout) {
    panic!("Memory allocation failed: {} bytes", layout.size());
}

// --- Constant Pool Helper Functions ---

fn add_utf8_constant(
    constant_pool: &mut Vec<u8>,
    constant_pool_count: &mut u16,
    text: &str,
) -> u16 {
    let index = *constant_pool_count;
    *constant_pool_count += 1;

    constant_pool.push(0x01); // CONSTANT_Utf8 tag
    let utf8_bytes = text.as_bytes();
    constant_pool.extend_from_slice(&(utf8_bytes.len() as u16).to_be_bytes());
    constant_pool.extend_from_slice(utf8_bytes);

    index
}

fn add_class_constant(
    constant_pool: &mut Vec<u8>,
    constant_pool_count: &mut u16,
    name_index: u16,
) -> u16 {
    let index = *constant_pool_count;
    *constant_pool_count += 1;

    constant_pool.push(0x07); // CONSTANT_Class tag
    constant_pool.extend_from_slice(&name_index.to_be_bytes());

    index
}

fn add_name_and_type_constant(
    constant_pool: &mut Vec<u8>,
    constant_pool_count: &mut u16,
    name_index: u16,
    descriptor_index: u16,
) -> u16 {
    let index = *constant_pool_count;
    *constant_pool_count += 1;

    constant_pool.push(0x0c); // CONSTANT_NameAndType tag
    constant_pool.extend_from_slice(&name_index.to_be_bytes());
    constant_pool.extend_from_slice(&descriptor_index.to_be_bytes());

    index
}

// Unused for now, useful for later
#[allow(dead_code)]
fn add_fieldref_constant(
    constant_pool: &mut Vec<u8>,
    constant_pool_count: &mut u16,
    class_index: u16,
    name_and_type_index: u16,
) -> u16 {
    let index = *constant_pool_count;
    *constant_pool_count += 1;

    constant_pool.push(0x09); // CONSTANT_Fieldref tag
    constant_pool.extend_from_slice(&class_index.to_be_bytes());
    constant_pool.extend_from_slice(&name_and_type_index.to_be_bytes());

    index
}

// Unused for now, useful for later
#[allow(dead_code)]
fn add_methodref_constant(
    constant_pool: &mut Vec<u8>,
    constant_pool_count: &mut u16,
    class_index: u16,
    name_and_type_index: u16,
) -> u16 {
    let index = *constant_pool_count;
    *constant_pool_count += 1;

    constant_pool.push(0x0a); // CONSTANT_Methodref tag
    constant_pool.extend_from_slice(&class_index.to_be_bytes());
    constant_pool.extend_from_slice(&name_and_type_index.to_be_bytes());

    index
}

// --- Improved helper function to convert Rust Ty to JVM descriptor ---
fn rust_ty_to_jvm_descriptor(rust_ty: Ty<'_>, _tcx: TyCtxt<'_>) -> String {
    use rustc_middle::ty::{TyKind, IntTy, UintTy, FloatTy};

    match rust_ty.kind() {
        // Primitive types
        TyKind::Bool => "Z".to_string(),       // JVM boolean
        TyKind::Char => "C".to_string(),       // JVM char

        // Signed integers mapped to JVM types or fallback to BigInteger for i128
        TyKind::Int(int_ty) => match int_ty {
            IntTy::I8    => "B".to_string(),    // JVM byte
            IntTy::I16   => "S".to_string(),    // JVM short
            IntTy::I32   => "I".to_string(),    // JVM int
            IntTy::I64   => "J".to_string(),    // JVM long
            IntTy::Isize => "I".to_string(),    // Fallback for isize
            IntTy::I128  => "Ljava/math/BigInteger;".to_string(), // No primitive for i128
        },

        // Unsigned integers mapped to JVM types or fallback to BigInteger for u128
        TyKind::Uint(uint_ty) => match uint_ty {
            UintTy::U8    => "B".to_string(),   // JVM byte
            UintTy::U16   => "S".to_string(),   // JVM short
            UintTy::U32   => "I".to_string(),   // JVM int
            UintTy::U64   => "J".to_string(),   // JVM long
            UintTy::Usize => "I".to_string(),   // Fallback for usize
            UintTy::U128  => "Ljava/math/BigInteger;".to_string(), // No primitive for u128
        },

        // Floating-point numbers mapped to appropriate JVM primitives.
        TyKind::Float(float_ty) => match float_ty {
            FloatTy::F32 => "F".to_string(),     // JVM float
            FloatTy::F64 => "D".to_string(),     // JVM double
            FloatTy::F16 => "F".to_string(),     // Fallback for half-precision float
            FloatTy::F128 => "D".to_string(),    // Fallback for extended precision
        },

        // Handle references: if it’s a string slice, map it to java.lang.String;
        // otherwise, use a generic object reference.
        TyKind::Ref(_, inner_ty, _) => {
            if let TyKind::Str = inner_ty.kind() {
                "Ljava/lang/String;".to_string()
            } else {
                "Ljava/lang/Object;".to_string()
            }
        },

        // For raw pointers, allow string pointers but panic otherwise.
        TyKind::RawPtr(ptr_ty, _) => {
            if let TyKind::Str = ptr_ty.kind() {
                "Ljava/lang/String;".to_string()
            } else {
                panic!("Pointers are not supported in Java.")
            }
        },

        // Map Rust string slices directly to java.lang.String
        TyKind::Str => "Ljava/lang/String;".to_string(),

        // Handle tuples: map the unit type () to void,
        // and for non-empty tuples, use a generic object (or consider a more specific mapping)
        TyKind::Tuple(tuple_fields) => {
            if tuple_fields.is_empty() {
                "V".to_string()
            } else {
                "Ljava/lang/Object;".to_string()
            }
        },

        // Map Rust's never type to void (even though it is conceptually different)
        TyKind::Never => "V".to_string(),

        // Fallback for any unhandled types
        _ => "Ljava/lang/Object;".to_string(),
    }
}

// --- MIR Visitor ---

struct MirToBytecodeVisitor<'tcx> {
    method_bytecode_instructions: Vec<u8>,
    function_name: String,    // Store function name
    tcx: TyCtxt<'tcx>,        // Store TyCtxt
    instance: Instance<'tcx>, // Store Instance
}

impl<'tcx> MirToBytecodeVisitor<'tcx> {
    fn new(
        method_bytecode_instructions: Vec<u8>,
        function_name: &str,
        tcx: TyCtxt<'tcx>,
        instance: Instance<'tcx>,
    ) -> Self {
        MirToBytecodeVisitor {
            method_bytecode_instructions,
            function_name: function_name.to_string(), // Store function name
            tcx,                                      // Store TyCtxt
            instance,                                 // Store Instance
        }
    }
}

impl Visitor<'_> for MirToBytecodeVisitor<'_> {
    fn visit_body(&mut self, body: &Body<'_>) {
        println!(
            "Visiting function body for function: {}...",
            self.function_name
        );
        self.super_body(body);
        println!(
            "...Finished visiting function body for function: {}.",
            self.function_name
        );
    }

    fn visit_basic_block_data(&mut self, block: BasicBlock, data: &BasicBlockData<'_>) {
        println!("  Visiting basic block: {block:?}");
        self.super_basic_block_data(block, data);
    }

    fn visit_statement(&mut self, statement: &Statement<'_>, location: Location) {
        println!(
            "    Visiting statement in block {:?}: {:?}",
            location.block, statement
        );
        if let StatementKind::Assign(box (_place, Rvalue::BinaryOp(bin_op, operands))) =
            &statement.kind
        {
            match bin_op {
                BinOp::Add | BinOp::AddWithOverflow => {
                    println!(
                        "      Found addition operation: {:?} + {:?}",
                        operands.0, operands.1
                    );

                    // --- Generate Java bytecode for iadd ---
                    // Load the first operand (argument 0)
                    self.method_bytecode_instructions.push(0x1a); // iload_0
                    // Load the second operand (argument 1)
                    self.method_bytecode_instructions.push(0x1b); // iload_1
                    // Perform integer addition
                    self.method_bytecode_instructions.push(0x60); // iadd
                    println!("      Generated bytecode: iload_0, iload_1, iadd");
                    // --- End bytecode generation ---
                }
                BinOp::Sub | BinOp::SubWithOverflow => {
                    println!(
                        "      Found subtraction operation: {:?} - {:?}",
                        operands.0, operands.1
                    );

                    // --- Generate Java bytecode for isub ---
                    // Load the first operand (argument 0)
                    self.method_bytecode_instructions.push(0x1a); // iload_0
                    // Load the second operand (argument 1)
                    self.method_bytecode_instructions.push(0x1b); // iload_1
                    // Perform integer subtraction
                    self.method_bytecode_instructions.push(0x64); // isub
                    println!("      Generated bytecode: iload_0, iload_1, isub");
                    // --- End bytecode generation ---
                }
                _ => {
                    println!("      Unsupported binary operation: {bin_op:?}");
                }
            }
        }
        self.super_statement(statement, location);
    }

    fn visit_terminator(&mut self, terminator: &Terminator<'_>, location: Location) {
        println!(
            "    Visiting terminator in block {:?}: {:?}",
            location.block, terminator
        );
        if terminator.kind == TerminatorKind::Return {
            println!(
                "      Found return terminator in function: {}",
                self.function_name
            );

            // Determine return type and generate appropriate bytecode
            let fn_sig = self.tcx.fn_sig(self.instance.def_id());
            let return_ty = fn_sig.skip_binder().output().skip_binder(); // Skip binder twice!
            let jvm_return_descriptor = rust_ty_to_jvm_descriptor(return_ty, self.tcx);

            match jvm_return_descriptor.as_str() {
                "V" => {
                    self.method_bytecode_instructions.push(0xb1); // _return for void
                    println!("      Generated bytecode: return (_return)");
                }
                "I" | "F" | "Z" | "B" | "C" | "S" => {
                    // Integer, Float, Boolean, Byte, Char, Short returns
                    self.method_bytecode_instructions.push(0xac); // ireturn (return integer value) - Correct return for i32, and others mapped to 'I'
                    println!("      Generated bytecode: ireturn");
                }
                "Ljava/lang/String;" | "Ljava/lang/Object;" => {
                    // Object returns (String, etc. for now)
                    self.method_bytecode_instructions.push(0xb0); // areturn (return object reference)
                    println!("      Generated bytecode: areturn");
                }
                _ => {
                    self.method_bytecode_instructions.push(0xb1); // default to void return if type is unknown or unsupported for now
                    println!(
                        "      Generated bytecode: return (_return) - default void return for unknown type"
                    );
                }
            }
        }
        self.super_terminator(terminator, location);
    }
}

fn generate_class_with_static_methods_bytecode(
    crate_name: &str,
    function_bytecodes: &FxIndexMap<String, Vec<u8>>,
    tcx: TyCtxt<'_>, // Take TyCtxt as argument
) -> Vec<u8> {
    let mut bytecode: Vec<u8> = Vec::new();

    // 1. Magic Number and Version (same as before)
    bytecode.extend_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
    bytecode.extend_from_slice(&[0x00, 0x00, 0x00, 0x34]);

    // 3. Constant Pool (modified)
    let mut constant_pool: Vec<u8> = Vec::new();
    let mut constant_pool_count = 1;

    // Class name is the crate name
    let class_name_index =
        add_utf8_constant(&mut constant_pool, &mut constant_pool_count, crate_name);
    let basic_class_index = add_class_constant(
        &mut constant_pool,
        &mut constant_pool_count,
        class_name_index,
    );

    // Superclass "java/lang/Object" (same as before)
    let object_class_name_index = add_utf8_constant(
        &mut constant_pool,
        &mut constant_pool_count,
        "java/lang/Object",
    );
    let object_class_index = add_class_constant(
        &mut constant_pool,
        &mut constant_pool_count,
        object_class_name_index,
    ); // Moved definition up

    // Constant pool entries for each function
    let mut method_constant_pool_indices = FxIndexMap::default(); // Store indices for each method

    for (function_name, _method_bytecode_instructions) in function_bytecodes {
        // Changed to borrow &function_bytecodes
        // Method name (e.g., "add")
        let method_name_index =
            add_utf8_constant(&mut constant_pool, &mut constant_pool_count, function_name);

        // Method descriptor - determine based on function signature, special case for "main"
        let instance =
            find_instance_by_name(tcx, function_name).expect("Instance not found for function");
        let fn_sig = tcx.fn_sig(instance.def_id());
        let mut method_descriptor = String::new();

        if function_name == "main" && fn_sig.skip_binder().inputs().skip_binder().is_empty() {
            // Check for main and no args
            method_descriptor = "([Ljava/lang/String;)V".to_string(); // Special main descriptor, needed as rust main = 0 args but java main expects an array of strings
        } else {
            // Regular descriptor generation
            method_descriptor.push('(');
            // Add argument descriptors
            for arg_ty in fn_sig.skip_binder().inputs().skip_binder() {
                method_descriptor.push_str(&rust_ty_to_jvm_descriptor(*arg_ty, tcx));
            }
            method_descriptor.push(')');

            // Add return descriptor
            let output_ty = fn_sig.skip_binder().output();
            method_descriptor.push_str(&rust_ty_to_jvm_descriptor(output_ty.skip_binder(), tcx));
        }

        let method_descriptor_index = add_utf8_constant(
            &mut constant_pool,
            &mut constant_pool_count,
            &method_descriptor,
        );

        // NameAndType for the method
        let method_name_and_type_index = add_name_and_type_constant(
            &mut constant_pool,
            &mut constant_pool_count,
            method_name_index,
            method_descriptor_index,
        );
        method_constant_pool_indices.insert(
            function_name.clone(),
            (
                method_name_index,
                method_descriptor_index,
                method_name_and_type_index,
            ),
        );
    }

    // "Code" attribute name (same as before)
    let code_attribute_name_index =
        add_utf8_constant(&mut constant_pool, &mut constant_pool_count, "Code");

    bytecode.extend_from_slice(&constant_pool_count.to_be_bytes()); // Constant pool count
    bytecode.extend_from_slice(&constant_pool);

    // 4. Access Flags, This Class, Superclass, Interfaces Count, Fields Count (same as before)
    bytecode.extend_from_slice(&[0x00, 0x21]); // Access flags (public class, ACC_SUPER)
    bytecode.extend_from_slice(&basic_class_index.to_be_bytes()); // This Class
    bytecode.extend_from_slice(&object_class_index.to_be_bytes()); // Superclass
    bytecode.extend_from_slice(&[0x00, 0x00]); // Interfaces Count
    bytecode.extend_from_slice(&[0x00, 0x00]); // Fields Count

    // 9. Methods Count (now based on the number of functions)
    bytecode.extend_from_slice(&(function_bytecodes.len() as u16).to_be_bytes());

    // --- Method Definitions for each function ---
    for (function_name, method_bytecode_instructions) in function_bytecodes {
        let (_method_name_index, method_descriptor_index, _method_name_and_type_index) =
            method_constant_pool_indices.get(function_name).unwrap();

        // --- Method Definition ---
        // Access flags (public static)
        bytecode.extend_from_slice(&[0x00, 0x09]); // ACC_PUBLIC, ACC_STATIC
        // Name index (index to function name - fetch from method_constant_pool_indices)
        let method_name_index_for_bytecode =
            method_constant_pool_indices.get(function_name).unwrap().0; // Get name index
        bytecode.extend_from_slice(&method_name_index_for_bytecode.to_be_bytes());
        // Descriptor index (fetch from method_constant_pool_indices)
        bytecode.extend_from_slice(&method_descriptor_index.to_be_bytes());
        // Attributes count (1, for Code attribute)
        bytecode.extend_from_slice(&[0x00, 0x01]);

        // --- Code Attribute for method ---
        // Attribute name index (index to "Code")
        bytecode.extend_from_slice(&code_attribute_name_index.to_be_bytes());
        // Attribute length (placeholder, updated later)
        bytecode.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

        let mut code_attribute_bytecode: Vec<u8> = Vec::new();
        // Calculate max stack size based on instructions
        let max_stack = calculate_max_stack_size(method_bytecode_instructions);
        code_attribute_bytecode.extend_from_slice(&max_stack.to_be_bytes());
        // Max locals (adjust based on function arguments and locals) - for main, 1 for args array
        let instance =
            find_instance_by_name(tcx, function_name).expect("Instance not found for function");
        let fn_sig = tcx.fn_sig(instance.def_id());
        let max_locals = fn_sig.skip_binder().inputs().skip_binder().len() as u16 + 1; // +1 for 'this' (even for static methods in JVM, slot 0 exists) // Skip binder twice!
        code_attribute_bytecode.extend_from_slice(&(max_locals as u16).to_be_bytes());
        // Code length (placeholder, updated later)
        let code_length_placeholder_index = code_attribute_bytecode.len();
        code_attribute_bytecode.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

        let code_length = method_bytecode_instructions.len() as u32;
        let code_length_bytes = code_length.to_be_bytes();
        code_attribute_bytecode[code_length_placeholder_index..code_length_placeholder_index + 4]
            .copy_from_slice(&code_length_bytes);
        code_attribute_bytecode.extend_from_slice(method_bytecode_instructions); // Use function's bytecode
        // Exception table length (0)
        code_attribute_bytecode.extend_from_slice(&[0x00, 0x00]);
        // Attributes count in Code attribute (0)
        code_attribute_bytecode.extend_from_slice(&[0x00, 0x00]);

        // Update attribute_length placeholder
        let attribute_length = code_attribute_bytecode.len() as u32;
        let attribute_length_bytes = attribute_length.to_be_bytes();
        let len = bytecode.len() - 4;
        bytecode[len..].copy_from_slice(&attribute_length_bytes);

        bytecode.extend_from_slice(&code_attribute_bytecode);
    }

    // 10. Attributes Count (0) (same as before)
    bytecode.extend_from_slice(&[0x00, 0x00]);

    bytecode
}

// Helper function to find Instance by function name (for descriptor generation)
fn find_instance_by_name<'tcx>(tcx: TyCtxt<'tcx>, function_name: &str) -> Option<Instance<'tcx>> {
    let module_items = tcx.hir_crate_items(());
    for item_id in module_items.free_items() {
        let item = tcx.hir_item(item_id);
        if let rustc_hir::ItemKind::Fn { ident, .. } = &item.kind {
            if ident.to_string() == function_name {
                let def_id = item_id.owner_id.to_def_id();
                return Some(Instance::mono(tcx, def_id));
            }
        }
    }
    None // Instance not found
}

fn calculate_max_stack_size(instructions: &[u8]) -> u16 {
    let mut stack_depth: i32 = 0;
    let mut max_stack_depth: i32 = 0;

    let mut instruction_pointer = 0;
    while instruction_pointer < instructions.len() {
        let opcode = instructions[instruction_pointer];
        instruction_pointer += 1;

        let stack_effect: i32 = match opcode {
            // 0x03 to 0x08 is iconst_m1, iconst_0, ..., iconst_5
            // 0x1a to 0x23 is iload_0, iload_1, ..., iload_3, iload, aload
            0x03..=0x08 | 0x1a..=0x23 => 1,
            0x60 | 0x64 => {
                stack_depth -= 1;
                0
            } // iadd, isub (pop 2, push 1 => net -1, but we're tracking *increase*, so net change from previous is -1+1=0 for tracking max)
            0xac | 0xb0 => {
                // ireturn, areturn
                stack_depth -= 1;
                0
            }
            _ => 0, // Default to 0 for unknown opcodes or those with no stack effect (i.e. _return - void return) for now
        };

        stack_depth += stack_effect;
        if stack_depth > max_stack_depth {
            max_stack_depth = stack_depth;
        }
        if stack_depth < 0 {
            stack_depth = 0; // Avoid negative stack depth if instructions are unbalanced (shouldn't happen in valid code)
        }
    }

    max_stack_depth.max(0) as u16 // Ensure max_stack is not negative, and cast to u16
}

struct RlibArchiveBuilder;
impl ArchiveBuilderBuilder for RlibArchiveBuilder {
    fn new_archive_builder<'a>(&self, sess: &'a Session) -> Box<dyn ArchiveBuilder + 'a> {
        Box::new(ArArchiveBuilder::new(
            sess,
            &rustc_codegen_ssa::back::archive::DEFAULT_OBJECT_READER,
        ))
    }
    fn create_dll_import_lib(
        &self,
        _sess: &Session,
        _lib_name: &str,
        _dll_imports: std::vec::Vec<rustc_codegen_ssa::back::archive::ImportLibraryItem>,
        _tmpdir: &Path,
    ) {
        unimplemented!("creating dll imports is not supported");
    }
}