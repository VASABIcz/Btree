#![feature(let_chains)]
#![feature(new_uninit)]
#![allow(soft_unstable)]
#![no_mangle]

use std::alloc::{alloc_zeroed, Layout};
use std::arch::x86_64::{_rdrand32_step, _rdrand64_step};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::hint::black_box;
use std::ops::{Deref, Range};
use std::process::Termination;
use std::sync::Arc;
use parking_lot::*;
use std::thread::Thread;
use std::time::Instant;

use arc_swap::{ArcSwap, Guard};

type ValuePtr = (Arc<Option<Frame>>, usize);

const FRAME: usize = 8;

struct ConstVec<const CAPACITY: usize, T> {
    ptr: *mut [T],
    length: usize
}

impl<const CAPACITY: usize, T> ConstVec<CAPACITY, T> {
    pub unsafe fn new() -> Self {
        Self {
            ptr: Box::into_raw(Box::<[T]>::new_uninit_slice(CAPACITY).assume_init()),
            length: 0,
        }
    }

    pub fn insert(&mut self, item: T, index: usize) {
        todo!()
    }

    pub fn first(&self) -> Option<&T> {
        todo!()
    }

    pub fn last(&self) -> Option<&T> {
        todo!()
    }

    pub fn push(&self, item: T) {
        todo!()
    }

    pub fn pop(&self) -> Option<T> {
        todo!()
    }
}

#[derive(Debug)]
pub struct Data {
    pub items: [JSON; FRAME],
    pub allocated: usize,
    pub freed: usize
}

pub struct Frame {
    pub next: ArcSwap<Option<Frame>>,
    pub previous: ArcSwap<Option<Frame>>,
    pub data: RwLock<Data>,
}

#[derive(Clone, Debug)]
pub enum JSON {
    Object(HashMap<String, JSON>),
    Array(Vec<JSON>),
    Str(String),
    Long(isize),
    Bool(bool),
    Double(f64),
    Null
}

struct IntNodeItems<T: Clone> {
    items: Vec<(isize, T)>
}

impl<T: Clone> IntNodeItems<T> {
    pub fn new() -> Self {
        Self {
            items: vec![]
        }
    }

    #[inline]
    pub fn newWith(item: (isize, T), capacity: usize) -> Self {
        let mut v = Vec::with_capacity(capacity);
        v.push(item);
        Self {
            items: v
        }
    }

    #[inline]
    pub fn getRange(&self) -> Option<Range<isize>> {
        let f = self.items.first()?;
        let s = self.items.last()?;

        Some(f.0..s.0)
    }

    #[inline]
    pub fn insert(&mut self, value: isize, src: T) {
        let mut index = self.items.len();

        // TODO binary search
        for item in self.items.iter().enumerate() {
            if item.1.0 >= value {
                index = item.0;
                break
            }
        }

        self.items.insert(index, (value, src));
    }

    #[inline]
    pub fn query(&self, range: &Range<isize>, buf: &mut Vec<T>) {
        for (value, item) in &self.items {
            if *value > range.end {
                return;
            }
            if range.contains(&value) {
                buf.push(item.clone())
            }
        }
    }

    #[inline]
    pub fn remove(&mut self, index: usize) {
        self.items.remove(index);
    }

    #[inline]
    pub fn removePredicate(&mut self, predicate: fn(&T) -> bool) {

    }
}

struct IntNode<const CAPACITY: usize, T: Clone> {
    items: RwLock<IntNodeItems<T>>,
    left: ArcSwap<Option<IntNode<CAPACITY, T>>>,
    right: ArcSwap<Option<IntNode<CAPACITY, T>>>
}

impl<const CAPACITY: usize, T: Clone> IntNode<CAPACITY, T> {
    pub fn new() -> Self {
        Self {
            items: RwLock::new(IntNodeItems::new()),
            left: Default::default(),
            right: Default::default(),
        }
    }

    #[inline]
    pub fn newWith(value: (isize, T), capacity: usize) -> Self {
        Self {
            items: RwLock::new(IntNodeItems::newWith(value,capacity)),
            left: Default::default(),
            right: Default::default(),
        }
    }

    #[inline]
    pub fn read(&self) -> RwLockReadGuard<IntNodeItems<T>> {
        self.items.read()
    }

    #[inline]
    pub fn getRange(&self) -> Option<Range<isize>> {
        let lock = self.read();
        lock.getRange().clone()
    }

    #[inline]
    pub fn length(&self) -> usize {
        let lock = self.read();
        lock.items.len()
    }

    #[inline]
    pub fn insertItem(&self, value: isize, src: T) {
        self.items.write().insert(value, src)
    }

    #[inline]
    pub fn pop(&self) -> Option<(isize, T)> {
        self.items.write().items.pop()
    }

    #[inline]
    pub fn insertOrCreate(node: &ArcSwap<Option<IntNode<CAPACITY, T>>>, value: isize, src: T) {
        match node.load().deref().deref() {
            None => {
                let newNode = IntNode::newWith((value, src), CAPACITY);
                node.swap(Arc::new(Some(newNode)));
            }
            Some(node) => {
                node.insert(value, src)
            }
        }
    }

    #[inline]
    pub fn insert(&self, value: isize, src: T) {
        let range = match self.getRange() {
            None => value-1..value+1,
            Some(v) => v
        };

        if range.contains(&value) || self.length() < CAPACITY {
            // value is in current node
            self.insertItem(value, src);
            if self.length() > CAPACITY {
                let v = self.pop().unwrap();
                Self::insertOrCreate(&self.right, v.0, v.1);
            }
        }
        else if range.end < value {
            // value is bigger than current node
            Self::insertOrCreate(&self.right, value, src);
        }
        else { // if range.start > value
            // value is smaller than current node
            Self::insertOrCreate(&self.left, value, src);
        }
    }

    #[inline]
    pub fn query(&self, range: &Range<isize>, buf: &mut Vec<T>) {
        self.read().query(range, buf)
    }

    #[inline]
    pub fn queryWithLock(lock: &RwLockReadGuard<IntNodeItems<T>>,range: &Range<isize>, buf: &mut Vec<T>) {
        lock.query(range, buf)
    }

    #[inline]
    pub fn findRange(&self, range: &Range<isize>, buf: &mut Vec<T>) {
        let lock = self.read();

        let nodeRange = match lock.getRange() {
            None => {
                return;
            },
            Some(v) => v
        };

        let checkLeft = nodeRange.start > range.start;
        let checkRight = nodeRange.end < range.end;

        if range.end > nodeRange.start && nodeRange.end > range.start {
            Self::queryWithLock(&lock, range, buf);
        }
        drop(lock);
        if checkRight {
            let a = self.right.load();
            match a.deref().deref() {
                None => {}
                Some(v) => {
                    v.findRange(range, buf);
                }
            }
        }
        if checkLeft {
            let a = self.left.load();
            match a.deref().deref() {
                None => {}
                Some(v) => {
                    v.findRange(range, buf);
                }
            }
        }
    }
}

fn main() {
    let mut i = 0;
    let root = IntNode::<64, ValuePtr>::new();

    {
        let now = Instant::now();

        {
            for _ in 0..1_000_000 {
                unsafe { _rdrand64_step(&mut i) };
                root.insert(i as isize, (Arc::new(None), 0));
            }
        }

        let elapsed = now.elapsed();
        println!("insert {elapsed:?}")
    }

    let r = Arc::new(root);

    {
        let now = Instant::now();

        let mut threads = vec![];
        for _ in 0..12 {
            let b = r.clone();
            let h = std::thread::spawn(move || {
                let mut buf = vec![];
                b.findRange(&((isize::MIN)..0), &mut buf);
            });
            threads.push(h);
        }

        for h in threads {
            h.join();
        }

        let elapsed = now.elapsed();
        println!("query 12 threads {elapsed:?}")
    }

    {
        let now = Instant::now();

        let mut buf = vec![];
        r.findRange(&((isize::MIN)..0), &mut buf);

        let elapsed = now.elapsed();
        println!("query 1 thread {elapsed:?} {}", buf.len())
    }

    /*
    {
        let now = Instant::now();

        {
            let mut x = vec![];
            let b = RwLock::new(Arc::<ValuePtr>::new((Arc::new(None), 0)));
            for  _ in 0..50_000_000 {
                x.push(b.read().unwrap().clone());
            }
            black_box(x);
        }

        let elapsed = now.elapsed();
        println!("test {elapsed:?}")
    }
     */
}