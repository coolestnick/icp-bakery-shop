#[macro_use]
extern crate serde;
use candid::{Decode, Encode};
use ic_cdk::api::time;
use ic_stable_structures::memory_manager::{MemoryId, MemoryManager, VirtualMemory};
use ic_stable_structures::{BoundedStorable, Cell, DefaultMemoryImpl, StableBTreeMap, Storable};
use std::{borrow::Cow, cell::RefCell};

type Memory = VirtualMemory<DefaultMemoryImpl>;
type IdCell = Cell<u64, Memory>;

#[derive(candid::CandidType, Clone, Serialize, Deserialize, Default)]
enum Category {
    #[default]
    Bakery,
    Cake,
    Cookies,
}

#[derive(candid::CandidType, Clone, Serialize, Deserialize, Default)]
struct Product {
    id: u64,
    name: String,
    category: Category,
    quantity: u32,
    created_at: u64,
    updated_at: Option<u64>,
}

// Implementing Storable for Product to convert to/from bytes for storage
impl Storable for Product {
    fn to_bytes(&self) -> std::borrow::Cow<[u8]> {
        Cow::Owned(Encode!(self).unwrap())
    }

    fn from_bytes(bytes: std::borrow::Cow<[u8]>) -> Self {
        Decode!(bytes.as_ref(), Self).unwrap()
    }
}

// Implementing BoundedStorable to define size limitations for Product storage
impl BoundedStorable for Product {
    const MAX_SIZE: u32 = 1024; // Maximum size for a Product in bytes
    const IS_FIXED_SIZE: bool = false;
}

thread_local! {
    static MEMORY_MANAGER: RefCell<MemoryManager<DefaultMemoryImpl>> = RefCell::new(
        MemoryManager::init(DefaultMemoryImpl::default())
    );

    static ID_COUNTER: RefCell<IdCell> = RefCell::new(
        IdCell::init(MEMORY_MANAGER.with(|m| m.borrow().get(MemoryId::new(0))), 0)
            .expect("Cannot create a counter")
    );

    static STORAGE: RefCell<StableBTreeMap<u64, Product, Memory>> =
        RefCell::new(StableBTreeMap::init(
            MEMORY_MANAGER.with(|m| m.borrow().get(MemoryId::new(1)))
    ));
}

// Product payload struct used to create or update a product
#[derive(candid::CandidType, Serialize, Deserialize, Default)]
struct ProductPayload {
    name: String,
    quantity: u32,
    category: Category,
}

// Payload for adding or removing stock
#[derive(candid::CandidType, Serialize, Deserialize, Default)]
struct StockPayload {
    amount: u32,
}

// Function to validate ProductPayload inputs
fn validate_product_payload(payload: &ProductPayload) -> Result<(), Error> {
    if payload.name.trim().is_empty() {
        return Err(Error::InvalidOperation {
            msg: "Product name cannot be empty.".to_string(),
        });
    }
    if payload.quantity == 0 {
        return Err(Error::InvalidOperation {
            msg: "Product quantity must be greater than zero.".to_string(),
        });
    }
    Ok(())
}

// Function to validate StockPayload inputs
fn validate_stock_payload(payload: &StockPayload) -> Result<(), Error> {
    if payload.amount == 0 {
        return Err(Error::InvalidOperation {
            msg: "Stock amount must be greater than zero.".to_string(),
        });
    }
    Ok(())
}

// Helper function to retrieve a product by its ID
fn _get_product(id: &u64) -> Option<Product> {
    STORAGE.with(|service| service.borrow().get(id))
}

// Query function to retrieve a product by ID
#[ic_cdk::query]
fn get_product(id: u64) -> Result<Product, Error> {
    match _get_product(&id) {
        Some(product) => Ok(product),
        None => Err(Error::NotFound {
            msg: format!("A product with id={} was not found", id),
        }),
    }
}

// Query function to get the current stock of a product by ID
#[ic_cdk::query]
fn get_stock(id: u64) -> Result<u32, Error> {
    match _get_product(&id) {
        Some(product) => Ok(product.quantity),
        None => Err(Error::NotFound {
            msg: format!("A product with id={} was not found", id),
        }),
    }
}

// Function to insert a product into the stable storage
fn do_insert(product: &Product) {
    STORAGE.with(|service| service.borrow_mut().insert(product.id, product.clone()));
}

// Function to add a new product to the storage
#[ic_cdk::update]
fn add_product(product: ProductPayload) -> Result<Product, Error> {
    // Validate payload before processing
    validate_product_payload(&product)?;

    // Generate a unique ID for the product
    let id = ID_COUNTER
        .with(|counter| {
            let current_value = *counter.borrow().get();
            counter.borrow_mut().set(current_value + 1)
        })
        .expect("Cannot increment id counter");
    
    // Create a new Product instance
    let item = Product {
        id,
        name: product.name,
        category: product.category, 
        quantity: product.quantity,
        created_at: time(),
        updated_at: None,
    };

    // Insert the new product into storage
    do_insert(&item);
    Ok(item)
}

// Function to update an existing product's details
#[ic_cdk::update]
fn update_product(id: u64, payload: ProductPayload) -> Result<Product, Error> {
    // Validate payload before processing
    validate_product_payload(&payload)?;

    // Update the product if it exists in storage
    match STORAGE.with(|service| service.borrow().get(&id)) {
        Some(mut product) => {
            product.name = payload.name;
            product.category = payload.category;
            product.quantity = payload.quantity;
            product.updated_at = Some(time());
            do_insert(&product);
            Ok(product)
        }
        None => Err(Error::NotFound {
            msg: format!("Couldn't update a product with id={}. Product not found", id),
        }),
    }
}

// Function to add stock to a product's quantity
#[ic_cdk::update]
fn add_quantity(id: u64, payload: StockPayload) -> Result<Product, Error> {
    // Validate the stock payload
    validate_stock_payload(&payload)?;

    match STORAGE.with(|service| service.borrow().get(&id)) {
        Some(mut product) => {
            product.quantity += payload.amount;
            product.updated_at = Some(time());
            do_insert(&product);
            Ok(product)
        }
        None => Err(Error::NotFound {
            msg: format!("Couldn't add quantity to product with id={}. Product not found", id),
        }),
    }
}

// Function to remove stock from a product's quantity
#[ic_cdk::update]
fn offload_quantity(id: u64, payload: StockPayload) -> Result<Product, Error> {
    // Validate the stock payload
    validate_stock_payload(&payload)?;

    match STORAGE.with(|service| service.borrow().get(&id)) {
        Some(mut product) => {
            if product.quantity == 0 {
                return Err(Error::InvalidOperation {
                    msg: format!("Product with id={} cannot be offloaded because the quantity is 0", id),
                });
            } else if payload.amount > product.quantity {
                return Err(Error::InvalidOperation {
                    msg: format!(
                        "Cannot offload more than available quantity. Available: {}, Trying to offload: {}",
                        product.quantity, payload.amount
                    ),
                });
            }
            product.quantity -= payload.amount;
            product.updated_at = Some(time());
            do_insert(&product);
            Ok(product)
        }
        None => Err(Error::NotFound {
            msg: format!("Couldn't offload a product with id={}. Product not found", id),
        }),
    }
}

// Function to remove a product from storage
#[ic_cdk::update]
fn remove_product(id: u64) -> Result<Product, Error> {
    match STORAGE.with(|service| service.borrow_mut().remove(&id)) {
        Some(product) => Ok(product),
        None => Err(Error::NotFound {
            msg: format!("Couldn't delete a product with id={}. Product not found", id),
        }),
    }
}

// Custom error handling enum
#[derive(candid::CandidType, Deserialize, Serialize)]
enum Error {
    NotFound { msg: String },
    InvalidOperation { msg: String },
}

// Export candid interface
ic_cdk::export_candid!();
