use futures::{pin_mut, ready, Stream, StreamExt};
use pin_project::{pin_project, project};
use serde::{Deserialize, Serialize};
use std::{
	collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, LinkedList, VecDeque}, hash::{BuildHasher, Hash}, marker::PhantomData, pin::Pin, task::{Context, Poll}
};

use super::{
	DistributedPipe, DistributedSink, Factory, Reducer, ReducerAsync, ReducerProcessSend, ReducerSend
};
use crate::pool::ProcessSend;

#[must_use]
pub struct Collect<I, A> {
	i: I,
	marker: PhantomData<fn() -> A>,
}
impl<I, A> Collect<I, A> {
	pub(crate) fn new(i: I) -> Self {
		Self {
			i,
			marker: PhantomData,
		}
	}
}

impl<I: DistributedPipe<Source>, Source, T: FromDistributedStream<I::Item>>
	DistributedSink<I, Source, T> for Collect<I, T>
{
	type ReduceAFactory = T::ReduceAFactory;
	type ReduceBFactory = T::ReduceBFactory;
	type ReduceA = T::ReduceA;
	type ReduceB = T::ReduceB;
	type ReduceC = T::ReduceC;

	fn reducers(self) -> (I, Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		let (a, b, c) = T::reducers();
		(self.i, a, b, c)
	}
}

pub trait FromDistributedStream<T>: Sized {
	type ReduceAFactory: Factory<Item = Self::ReduceA> + Clone + ProcessSend;
	type ReduceBFactory: Factory<Item = Self::ReduceB>;
	type ReduceA: ReducerSend<Item = T> + ProcessSend;
	type ReduceB: ReducerProcessSend<Item = <Self::ReduceA as Reducer>::Output> + ProcessSend;
	type ReduceC: Reducer<Item = <Self::ReduceB as Reducer>::Output, Output = Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC);
	// 	fn from_dist_stream<I>(dist_stream: I, pool: &Pool) -> Self where T: Serialize + DeserializeOwned + Send + 'static, I: IntoDistributedStream<Item = T>, <<I as IntoDistributedStream>::Iter as DistributedStream>::Task: Serialize + DeserializeOwned + Send + 'static;
}

#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct DefaultReduceFactory<T>(PhantomData<fn(T)>);
impl<T> DefaultReduceFactory<T> {
	pub fn new() -> Self {
		Self(PhantomData)
	}
}
impl<T> Default for DefaultReduceFactory<T> {
	fn default() -> Self {
		Self(PhantomData)
	}
}
impl<T: Default + Reducer> Factory for DefaultReduceFactory<T> {
	type Item = T;
	fn make(&self) -> Self::Item {
		T::default()
	}
}
impl<T> Clone for DefaultReduceFactory<T> {
	fn clone(&self) -> Self {
		Self(PhantomData)
	}
}

pub trait Push<A> {
	fn push(&mut self, item: A);
}
// impl<A, T: Extend<A>> Push<A> for T {
// 	default fn push(&mut self, item: A) {
// 		self.extend(iter::once(item));
// 	}
// }
impl<T> Push<T> for Vec<T> {
	#[inline]
	fn push(&mut self, item: T) {
		self.push(item);
	}
}
impl<T> Push<T> for LinkedList<T> {
	#[inline]
	fn push(&mut self, item: T) {
		self.push_back(item);
	}
}
impl<T, S> Push<T> for HashSet<T, S>
where
	T: Eq + Hash,
	S: BuildHasher,
{
	#[inline]
	fn push(&mut self, item: T) {
		let _ = self.insert(item);
	}
}
impl<K, V, S> Push<(K, V)> for HashMap<K, V, S>
where
	K: Eq + Hash,
	S: BuildHasher,
{
	#[inline]
	fn push(&mut self, item: (K, V)) {
		let _ = self.insert(item.0, item.1);
	}
}
impl<T> Push<T> for BTreeSet<T>
where
	T: Ord,
{
	#[inline]
	fn push(&mut self, item: T) {
		let _ = self.insert(item);
	}
}
impl<K, V> Push<(K, V)> for BTreeMap<K, V>
where
	K: Ord,
{
	#[inline]
	fn push(&mut self, item: (K, V)) {
		let _ = self.insert(item.0, item.1);
	}
}
impl Push<char> for String {
	#[inline]
	fn push(&mut self, item: char) {
		self.push(item);
	}
}
impl Push<Self> for String {
	#[inline]
	fn push(&mut self, item: Self) {
		self.push_str(&item);
	}
}
impl Push<Self> for () {
	#[inline]
	fn push(&mut self, _item: Self) {}
}

#[pin_project]
#[derive(Serialize, Deserialize)]
#[serde(
	bound(serialize = "T: Serialize"),
	bound(deserialize = "T: Deserialize<'de>")
)]
pub struct PushReducer<A, T = A>(Option<T>, PhantomData<fn(A)>);
impl<A, T> PushReducer<A, T> {
	pub(crate) fn new(t: T) -> Self {
		Self(Some(t), PhantomData)
	}
}
impl<A, T> Default for PushReducer<A, T>
where
	T: Default,
{
	fn default() -> Self {
		Self(Some(T::default()), PhantomData)
	}
}
impl<A, T: Push<A>> Reducer for PushReducer<A, T> {
	type Item = A;
	type Output = T;
	type Async = Self;

	fn into_async(self) -> Self::Async {
		self
	}
}
impl<A, T: Push<A>> ReducerAsync for PushReducer<A, T> {
	type Item = A;
	type Output = T;

	#[inline]
	fn poll_forward(
		self: Pin<&mut Self>, cx: &mut Context,
		mut stream: Pin<&mut impl Stream<Item = Self::Item>>,
	) -> Poll<()> {
		let self_ = self.project();
		while let Some(item) = ready!(stream.as_mut().poll_next(cx)) {
			self_.0.as_mut().unwrap().push(item);
		}
		Poll::Ready(())
	}
	fn poll_output(mut self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Self::Output> {
		Poll::Ready(self.0.take().unwrap())
	}
}
impl<A, T: Push<A>> ReducerProcessSend for PushReducer<A, T>
where
	A: 'static,
	T: ProcessSend,
{
	type Output = T;
}
impl<A, T: Push<A>> ReducerSend for PushReducer<A, T>
where
	A: 'static,
	T: Send + 'static,
{
	type Output = T;
}

#[pin_project]
#[derive(Serialize, Deserialize)]
#[serde(
	bound(serialize = "T: Serialize"),
	bound(deserialize = "T: Deserialize<'de>")
)]
pub struct ExtendReducer<A, T = A>(Option<T>, PhantomData<fn(A)>);
impl<A, T> Default for ExtendReducer<A, T>
where
	T: Default,
{
	fn default() -> Self {
		Self(Some(T::default()), PhantomData)
	}
}
impl<A: IntoIterator<Item = B>, T: Extend<B>, B> ReducerProcessSend for ExtendReducer<A, T>
where
	A: 'static,
	T: ProcessSend,
{
	type Output = T;
}
impl<A: IntoIterator<Item = B>, T: Extend<B>, B> ReducerSend for ExtendReducer<A, T>
where
	A: 'static,
	T: Send + 'static,
{
	type Output = T;
}
impl<A: IntoIterator<Item = B>, T: Extend<B>, B> Reducer for ExtendReducer<A, T> {
	type Item = A;
	type Output = T;
	type Async = Self;

	fn into_async(self) -> Self::Async {
		self
	}
}
impl<A: IntoIterator<Item = B>, T: Extend<B>, B> ReducerAsync for ExtendReducer<A, T> {
	type Item = A;
	type Output = T;

	#[inline]
	fn poll_forward(
		self: Pin<&mut Self>, cx: &mut Context,
		mut stream: Pin<&mut impl Stream<Item = Self::Item>>,
	) -> Poll<()> {
		let self_ = self.project();
		while let Some(item) = ready!(stream.as_mut().poll_next(cx)) {
			self_.0.as_mut().unwrap().extend(item);
		}
		Poll::Ready(())
	}
	fn poll_output(mut self: Pin<&mut Self>, _cx: &mut Context) -> Poll<Self::Output> {
		Poll::Ready(self.0.take().unwrap())
	}
}

#[pin_project]
pub struct IntoReducer<R, T>(#[pin] R, PhantomData<fn(T)>);
impl<R, T> Default for IntoReducer<R, T>
where
	R: Default,
{
	fn default() -> Self {
		Self(R::default(), PhantomData)
	}
}
impl<R: Reducer, T> Reducer for IntoReducer<R, T>
where
	R::Output: Into<T>,
{
	type Item = R::Item;
	type Output = T;
	type Async = IntoReducer<R::Async, T>;

	fn into_async(self) -> Self::Async {
		IntoReducer(self.0.into_async(), PhantomData)
	}
}
impl<R: ReducerAsync, T> ReducerAsync for IntoReducer<R, T>
where
	R::Output: Into<T>,
{
	type Item = R::Item;
	type Output = T;

	#[inline]
	fn poll_forward(
		self: Pin<&mut Self>, cx: &mut Context, stream: Pin<&mut impl Stream<Item = Self::Item>>,
	) -> Poll<()> {
		let stream = stream.map(Into::into);
		pin_mut!(stream);
		self.project().0.poll_forward(cx, stream)
	}
	fn poll_output(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
		self.project().0.poll_output(cx).map(Into::into)
	}
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OptionReduceFactory<RF: Factory>(RF);
impl<RF: Factory> Factory for OptionReduceFactory<RF> {
	type Item = OptionReducer<RF::Item>;

	fn make(&self) -> Self::Item {
		OptionReducer(Some(self.0.make()))
	}
}

#[pin_project]
#[derive(Serialize, Deserialize)]
pub struct OptionReducer<R>(#[pin] Option<R>);
impl<R> Default for OptionReducer<R>
where
	R: Default,
{
	fn default() -> Self {
		Self(Some(R::default()))
	}
}
impl<R: Reducer> Reducer for OptionReducer<R> {
	type Item = Option<R::Item>;
	type Output = Option<R::Output>;
	type Async = OptionReducer<R::Async>;

	fn into_async(self) -> Self::Async {
		OptionReducer(Some(self.0.unwrap().into_async()))
	}
}
impl<R: ReducerAsync> ReducerAsync for OptionReducer<R> {
	type Item = Option<R::Item>;
	type Output = Option<R::Output>;

	#[inline]
	fn poll_forward(
		self: Pin<&mut Self>, _cx: &mut Context, _stream: Pin<&mut impl Stream<Item = Self::Item>>,
	) -> Poll<()> {
		todo!()
		// let self_ = self.project();
		// match (self_.0, item.is_some()) {
		// 	(&mut Some(ref mut a), true) => {
		// 		return a.push(item.unwrap());
		// 	}
		// 	(self_, _) => *self_ = None,
		// }
		// self_.0.is_some()
	}
	fn poll_output(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
		self.project()
			.0
			.as_pin_mut()
			.map(|r| r.poll_output(cx).map(Some))
			.unwrap_or(Poll::Ready(None))
	}
}
impl<R: Reducer> ReducerProcessSend for OptionReducer<R>
where
	R: ProcessSend,
	R::Output: ProcessSend,
{
	type Output = Option<R::Output>;
}
impl<R: Reducer> ReducerSend for OptionReducer<R>
where
	R: Send + 'static,
	R::Output: Send + 'static,
{
	type Output = Option<R::Output>;
}

#[derive(Serialize, Deserialize)]
#[serde(
	bound(serialize = "RF: Serialize"),
	bound(deserialize = "RF: Deserialize<'de>")
)]
pub struct ResultReduceFactory<RF, E>(RF, PhantomData<fn(E)>);
impl<RF, E> Factory for ResultReduceFactory<RF, E>
where
	RF: Factory,
{
	type Item = ResultReducer<RF::Item, E>;

	fn make(&self) -> Self::Item {
		ResultReducer(self.0.make(), PhantomData)
	}
}
impl<RF, E> Clone for ResultReduceFactory<RF, E>
where
	RF: Clone,
{
	fn clone(&self) -> Self {
		Self(self.0.clone(), PhantomData)
	}
}

#[derive(Serialize, Deserialize)]
pub struct ResultReducer<R, E>(R, PhantomData<E>);
impl<R, E> Default for ResultReducer<R, E>
where
	R: Default,
{
	fn default() -> Self {
		Self(R::default(), PhantomData)
	}
}
impl<R: Reducer, E> Reducer for ResultReducer<R, E> {
	type Item = Result<R::Item, E>;
	type Output = Result<R::Output, E>;
	type Async = ResultReducerAsync<R::Async, E>;

	fn into_async(self) -> Self::Async {
		ResultReducerAsync::Ok(self.0.into_async())
	}
}
#[pin_project]
pub enum ResultReducerAsync<R, E> {
	Ok(#[pin] R),
	Err(Option<E>),
}
impl<R: ReducerAsync, E> ReducerAsync for ResultReducerAsync<R, E> {
	type Item = Result<R::Item, E>;
	type Output = Result<R::Output, E>;

	#[inline]
	fn poll_forward(
		self: Pin<&mut Self>, _cx: &mut Context, _stream: Pin<&mut impl Stream<Item = Self::Item>>,
	) -> Poll<()> {
		todo!()
		// let self_ = self.project();
		// match (self_.0, item.is_ok()) {
		// 	(&mut Ok(ref mut a), true) => {
		// 		return a.push(item.ok().unwrap());
		// 	}
		// 	(self_, false) => *self_ = Err(item.err().unwrap()),
		// 	_ => (),
		// }
		// self_.0.is_ok()
	}
	#[project]
	fn poll_output(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
		#[project]
		match self.project() {
			ResultReducerAsync::Ok(a) => a.poll_output(cx).map(Ok),
			ResultReducerAsync::Err(b) => Poll::Ready(Err(b.take().unwrap())),
		}
	}
}
impl<R: Reducer, E> ReducerProcessSend for ResultReducer<R, E>
where
	R: ProcessSend,
	R::Output: ProcessSend,
	E: ProcessSend,
{
	type Output = Result<R::Output, E>;
}
impl<R: Reducer, E> ReducerSend for ResultReducer<R, E>
where
	R: Send + 'static,
	R::Output: Send + 'static,
	E: Send + 'static,
{
	type Output = Result<R::Output, E>;
}

impl<T> FromDistributedStream<T> for Vec<T>
where
	T: ProcessSend,
{
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<T, Self>;
	type ReduceB = ExtendReducer<Self>;
	type ReduceC = ExtendReducer<Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl<T> FromDistributedStream<T> for VecDeque<T>
where
	T: ProcessSend,
{
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<T, Vec<T>>;
	type ReduceB = ExtendReducer<Vec<T>>;
	type ReduceC = IntoReducer<ExtendReducer<Vec<T>>, Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl<T: Ord> FromDistributedStream<T> for BinaryHeap<T>
where
	T: ProcessSend,
{
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<T, Vec<T>>;
	type ReduceB = ExtendReducer<Vec<T>>;
	type ReduceC = IntoReducer<ExtendReducer<Vec<T>>, Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl<T> FromDistributedStream<T> for LinkedList<T>
where
	T: ProcessSend,
{
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<T, Self>;
	type ReduceB = ExtendReducer<Self>;
	type ReduceC = ExtendReducer<Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl<T, S> FromDistributedStream<T> for HashSet<T, S>
where
	T: Eq + Hash + ProcessSend,
	S: BuildHasher + Default + Send + 'static,
{
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<T, Self>;
	type ReduceB = ExtendReducer<Self>;
	type ReduceC = ExtendReducer<Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl<K, V, S> FromDistributedStream<(K, V)> for HashMap<K, V, S>
where
	K: Eq + Hash + ProcessSend,
	V: ProcessSend,
	S: BuildHasher + Default + Send + 'static,
{
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<(K, V), Self>;
	type ReduceB = ExtendReducer<Self>;
	type ReduceC = ExtendReducer<Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl<T> FromDistributedStream<T> for BTreeSet<T>
where
	T: Ord + ProcessSend,
{
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<T, Self>;
	type ReduceB = ExtendReducer<Self>;
	type ReduceC = ExtendReducer<Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl<K, V> FromDistributedStream<(K, V)> for BTreeMap<K, V>
where
	K: Ord + ProcessSend,
	V: ProcessSend,
{
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<(K, V), Self>;
	type ReduceB = ExtendReducer<Self>;
	type ReduceC = ExtendReducer<Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl FromDistributedStream<char> for String {
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<char, Self>;
	type ReduceB = PushReducer<Self>;
	type ReduceC = PushReducer<Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl FromDistributedStream<Self> for String {
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<Self>;
	type ReduceB = PushReducer<Self>;
	type ReduceC = PushReducer<Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl FromDistributedStream<()> for () {
	type ReduceAFactory = DefaultReduceFactory<Self::ReduceA>;
	type ReduceBFactory = DefaultReduceFactory<Self::ReduceB>;
	type ReduceA = PushReducer<Self>;
	type ReduceB = PushReducer<Self>;
	type ReduceC = PushReducer<Self>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		Default::default()
	}
}

impl<T, C: FromDistributedStream<T>> FromDistributedStream<Option<T>> for Option<C> {
	type ReduceAFactory = OptionReduceFactory<C::ReduceAFactory>;
	type ReduceBFactory = OptionReduceFactory<C::ReduceBFactory>;
	type ReduceA = OptionReducer<C::ReduceA>;
	type ReduceB = OptionReducer<C::ReduceB>;
	type ReduceC = OptionReducer<C::ReduceC>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		let (a, b, c) = C::reducers();
		(
			OptionReduceFactory(a),
			OptionReduceFactory(b),
			OptionReducer(Some(c)),
		)
	}
}

impl<T, C: FromDistributedStream<T>, E> FromDistributedStream<Result<T, E>> for Result<C, E>
where
	E: ProcessSend,
{
	type ReduceAFactory = ResultReduceFactory<C::ReduceAFactory, E>;
	type ReduceBFactory = ResultReduceFactory<C::ReduceBFactory, E>;
	type ReduceA = ResultReducer<C::ReduceA, E>;
	type ReduceB = ResultReducer<C::ReduceB, E>;
	type ReduceC = ResultReducer<C::ReduceC, E>;

	fn reducers() -> (Self::ReduceAFactory, Self::ReduceBFactory, Self::ReduceC) {
		let (a, b, c) = C::reducers();
		(
			ResultReduceFactory(a, PhantomData),
			ResultReduceFactory(b, PhantomData),
			ResultReducer(c, PhantomData),
		)
	}
}